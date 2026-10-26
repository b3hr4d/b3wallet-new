//! The guard's forward pass: BERT for one request (no batch, no padding, no dropout), copied from candid-profile's
//! W8 tagger (`packages/command-model/tagger/src/model.rs`), whose model the guard's `guard/model.py` copied. Only
//! the SIMD gating differs: there it follows the build's target features; here every Wasm build runs the f32x4
//! loops, since every Internet Computer replica runs Wasm SIMD, and native builds run the scalar ones.
//!
//! Linear weights are stored [in][out] (the export transposes them), so 16 neighbouring outputs read 16 contiguous
//! weights for each input (`block16`). The f32x4 loops do the same multiplications and additions, lane by lane in
//! the same order, as the scalar loops, so both give the same bits.

use serde_json::Value;

pub struct Config {
    pub hidden: usize,
    pub layers: usize,
    pub heads: usize,
    pub intermediate: usize,
    pub eps: f32,
    pub max_len: usize,
}

struct Linear {
    w: std::ops::Range<usize>,
    b: std::ops::Range<usize>,
    n_in: usize,
    n_out: usize,
}

struct Norm {
    w: std::ops::Range<usize>,
    b: std::ops::Range<usize>,
}

struct Layer {
    query: Linear,
    key: Linear,
    value: Linear,
    attn_out: Linear,
    attn_norm: Norm,
    ffn_in: Linear,
    ffn_out: Linear,
    ffn_norm: Norm,
}

pub struct Model {
    pub config: Config,
    data: Vec<f32>,
    word: std::ops::Range<usize>,
    position: std::ops::Range<usize>,
    token_type: std::ops::Range<usize>,
    emb_norm: Norm,
    layers: Vec<Layer>,
    tool_head: Linear,
    slot_head: Linear,
}

/// The tensor table of the file header: name -> (offset, shape).
struct Table<'a>(Vec<(&'a str, usize, Vec<usize>)>, usize);

impl Table<'_> {
    fn get(&self, name: &str, shape: &[usize]) -> Result<std::ops::Range<usize>, String> {
        let (_, offset, found) = self
            .0
            .iter()
            .find(|t| t.0 == name)
            .ok_or(format!("no tensor {name}"))?;
        if found != shape {
            return Err(format!("{name}: shape {found:?}, expected {shape:?}"));
        }
        let end = offset + shape.iter().product::<usize>();
        if end > self.1 {
            return Err(format!("{name}: past the end of the data"));
        }
        Ok(*offset..end)
    }

    fn linear(&self, name: &str, n_in: usize, n_out: usize) -> Result<Linear, String> {
        Ok(Linear {
            w: self.get(&format!("{name}.weight_t"), &[n_in, n_out])?,
            b: self.get(&format!("{name}.bias"), &[n_out])?,
            n_in,
            n_out,
        })
    }

    fn norm(&self, name: &str, n: usize) -> Result<Norm, String> {
        Ok(Norm {
            w: self.get(&format!("{name}.weight"), &[n])?,
            b: self.get(&format!("{name}.bias"), &[n])?,
        })
    }
}

fn usize_of(v: &Value, key: &str) -> Result<usize, String> {
    v[key]
        .as_u64()
        .map(|n| n as usize)
        .ok_or(format!("config.{key} is missing"))
}

impl Model {
    /// The model from the header's `config` and `tensors` and the f32 data that follows the header.
    pub fn new(
        header: &Value,
        n_tools: usize,
        n_labels: usize,
        n_vocab: usize,
        data: Vec<f32>,
    ) -> Result<Model, String> {
        let c = &header["config"];
        let config = Config {
            hidden: usize_of(c, "hidden")?,
            layers: usize_of(c, "layers")?,
            heads: usize_of(c, "heads")?,
            intermediate: usize_of(c, "intermediate")?,
            eps: c["eps"].as_f64().ok_or("config.eps is missing")? as f32,
            max_len: usize_of(c, "max_len")?,
        };
        let mut rows = Vec::new();
        for t in header["tensors"].as_array().ok_or("no tensor table")? {
            let name = t["name"].as_str().ok_or("a tensor has no name")?;
            let offset = t["offset"].as_u64().ok_or("a tensor has no offset")? as usize;
            let shape = t["shape"].as_array().ok_or("a tensor has no shape")?;
            let shape = shape
                .iter()
                .map(|d| d.as_u64().map(|d| d as usize))
                .collect::<Option<Vec<_>>>()
                .ok_or("bad shape")?;
            rows.push((name, offset, shape));
        }
        let table = Table(rows, data.len());
        let (h, f) = (config.hidden, config.intermediate);
        if config.heads == 0 || h % config.heads != 0 {
            return Err("hidden is not a multiple of heads".into());
        }
        let mut layers = Vec::new();
        for i in 0..config.layers {
            let p = format!("layers.{i}.");
            layers.push(Layer {
                query: table.linear(&format!("{p}query"), h, h)?,
                key: table.linear(&format!("{p}key"), h, h)?,
                value: table.linear(&format!("{p}value"), h, h)?,
                attn_out: table.linear(&format!("{p}attn_out"), h, h)?,
                attn_norm: table.norm(&format!("{p}attn_norm"), h)?,
                ffn_in: table.linear(&format!("{p}ffn_in"), h, f)?,
                ffn_out: table.linear(&format!("{p}ffn_out"), f, h)?,
                ffn_norm: table.norm(&format!("{p}ffn_norm"), h)?,
            });
        }
        Ok(Model {
            word: table.get("word", &[n_vocab, h])?,
            position: table.get("position", &[config.max_len, h])?,
            token_type: table.get("token_type", &[2, h])?,
            emb_norm: table.norm("emb_norm", h)?,
            layers,
            tool_head: table.linear("tool_head", h, n_tools)?,
            slot_head: table.linear("slot_head", h, n_labels)?,
            config,
            data,
        })
    }

    /// y[t] = x[t] W + b for each of the n rows of x. Outputs are computed 16 at a time (`block16`); the last
    /// n_out % 16 one by one, with the same order of sums.
    fn linear(&self, l: &Linear, x: &[f32], n: usize) -> Vec<f32> {
        let w = &self.data[l.w.clone()];
        let b = &self.data[l.b.clone()];
        let (n_in, n_out) = (l.n_in, l.n_out);
        let mut y = vec![0f32; n * n_out];
        for t in 0..n {
            let xs = &x[t * n_in..(t + 1) * n_in];
            let row = &mut y[t * n_out..(t + 1) * n_out];
            let mut j = 0;
            while j + 16 <= n_out {
                row[j..j + 16].copy_from_slice(&block16(xs, w, n_out, j));
                j += 16;
            }
            for (k, out) in row.iter_mut().enumerate().skip(j) {
                *out = xs
                    .iter()
                    .enumerate()
                    .fold(0f32, |a, (i, &xi)| a + xi * w[i * n_out + k]);
            }
            for (out, &bj) in row.iter_mut().zip(b) {
                *out += bj;
            }
        }
        y
    }

    fn norm(&self, nrm: &Norm, x: &mut [f32]) {
        let h = self.config.hidden;
        let w = &self.data[nrm.w.clone()];
        let b = &self.data[nrm.b.clone()];
        for row in x.chunks_exact_mut(h) {
            let mean = row.iter().sum::<f32>() / h as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / h as f32;
            let inv = 1.0 / libm::sqrtf(var + self.config.eps);
            for ((v, &wj), &bj) in row.iter_mut().zip(w).zip(b) {
                *v = (*v - mean) * inv * wj + bj;
            }
        }
    }

    /// Tool logits, and the BIO logits of every position ([CLS] and [SEP] included), for `ids` with token types
    /// `types` (0 or 1). `ids` must fit (`max_len`), as `Guard::answer` ensures.
    pub fn forward(&self, ids: &[u32], types: &[u32]) -> (Vec<f32>, Vec<f32>) {
        let (h, n) = (self.config.hidden, ids.len());
        let word = &self.data[self.word.clone()];
        let position = &self.data[self.position.clone()];
        let token_type = &self.data[self.token_type.clone()];
        let mut x = vec![0f32; n * h];
        for (t, (&id, &ty)) in ids.iter().zip(types).enumerate() {
            let w = &word[id as usize * h..(id as usize + 1) * h];
            let p = &position[t * h..(t + 1) * h];
            let tt = &token_type[ty as usize * h..(ty as usize + 1) * h];
            for (j, v) in x[t * h..(t + 1) * h].iter_mut().enumerate() {
                *v = w[j] + p[j] + tt[j];
            }
        }
        self.norm(&self.emb_norm, &mut x);

        let heads = self.config.heads;
        let dh = h / heads;
        let scale = libm::sqrtf(dh as f32);
        let mut scores = vec![0f32; n];
        let mut kt = vec![0f32; dh * n];
        for layer in &self.layers {
            let q = self.linear(&layer.query, &x, n);
            let k = self.linear(&layer.key, &x, n);
            let v = self.linear(&layer.value, &x, n);
            let mut context = vec![0f32; n * h];
            for head in 0..heads {
                let cols = head * dh..(head + 1) * dh;
                // This head's keys, transposed to [dh][n]: the scores of one query are then dh `axpy`s over n.
                for s in 0..n {
                    for (d, &kd) in k[s * h..(s + 1) * h][cols.clone()].iter().enumerate() {
                        kt[d * n + s] = kd;
                    }
                }
                for t in 0..n {
                    scores.fill(0.0);
                    for (d, &qd) in q[t * h..(t + 1) * h][cols.clone()].iter().enumerate() {
                        axpy(&mut scores, qd, &kt[d * n..(d + 1) * n]);
                    }
                    let mut max = f32::NEG_INFINITY;
                    for score in scores.iter_mut() {
                        *score /= scale;
                        max = max.max(*score);
                    }
                    let mut sum = 0f32;
                    for score in scores.iter_mut() {
                        *score = libm::expf(*score - max);
                        sum += *score;
                    }
                    let ct = &mut context[t * h..(t + 1) * h][cols.clone()];
                    for (s, &p) in scores.iter().enumerate() {
                        axpy(ct, p / sum, &v[s * h..(s + 1) * h][cols.clone()]);
                    }
                }
            }
            let mut attended = self.linear(&layer.attn_out, &context, n);
            for (a, &xi) in attended.iter_mut().zip(&x) {
                *a += xi;
            }
            self.norm(&layer.attn_norm, &mut attended);
            x = attended;

            let mut inner = self.linear(&layer.ffn_in, &x, n);
            for u in inner.iter_mut() {
                *u = 0.5 * *u * (1.0 + libm::erff(*u * std::f32::consts::FRAC_1_SQRT_2));
            }
            let mut out = self.linear(&layer.ffn_out, &inner, n);
            for (o, &xi) in out.iter_mut().zip(&x) {
                *o += xi;
            }
            self.norm(&layer.ffn_norm, &mut out);
            x = out;
        }
        (
            self.linear(&self.tool_head, &x[..h], 1),
            self.linear(&self.slot_head, &x, n),
        )
    }
}

/// Sixteen outputs of x W, from column j: acc[k] = sum over i, in order, of xs[i] * w[i][j + k]. With `simd128`
/// the sixteen sums are four f32x4 accumulators; each lane does the same multiplications and additions, in the
/// same order, as the scalar version, so both give the same bits.
#[cfg(target_arch = "wasm32")]
fn block16(xs: &[f32], w: &[f32], n_out: usize, j: usize) -> [f32; 16] {
    // simd128 is part of the Wasm every Internet Computer replica runs; a Wasm engine without it refuses the
    // module, so calling it is safe.
    block16_simd(xs, w, n_out, j)
}

#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn block16_simd(xs: &[f32], w: &[f32], n_out: usize, j: usize) -> [f32; 16] {
    use core::arch::wasm32::{f32x4_add, f32x4_mul, f32x4_splat, v128, v128_load, v128_store};
    assert!(j + 16 <= n_out && xs.len() * n_out <= w.len());
    let zero = f32x4_splat(0.0);
    let (mut a0, mut a1, mut a2, mut a3) = (zero, zero, zero, zero);
    let mut row = w[j..].as_ptr();
    for &xi in xs {
        let xv = f32x4_splat(xi);
        // SAFETY: row points at w[i * n_out + j], and the assert above keeps its 16 values inside w; v128_load
        // takes unaligned addresses.
        unsafe {
            a0 = f32x4_add(a0, f32x4_mul(xv, v128_load(row as *const v128)));
            a1 = f32x4_add(a1, f32x4_mul(xv, v128_load(row.add(4) as *const v128)));
            a2 = f32x4_add(a2, f32x4_mul(xv, v128_load(row.add(8) as *const v128)));
            a3 = f32x4_add(a3, f32x4_mul(xv, v128_load(row.add(12) as *const v128)));
            row = row.wrapping_add(n_out);
        }
    }
    let mut out = [0f32; 16];
    // SAFETY: out holds 16 values; v128_store takes unaligned addresses.
    unsafe {
        let p = out.as_mut_ptr();
        v128_store(p as *mut v128, a0);
        v128_store(p.add(4) as *mut v128, a1);
        v128_store(p.add(8) as *mut v128, a2);
        v128_store(p.add(12) as *mut v128, a3);
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn block16(xs: &[f32], w: &[f32], n_out: usize, j: usize) -> [f32; 16] {
    let mut acc = [0f32; 16];
    for (i, &xi) in xs.iter().enumerate() {
        let wr: &[f32; 16] = w[i * n_out + j..i * n_out + j + 16]
            .try_into()
            .expect("16 values");
        for k in 0..16 {
            acc[k] += xi * wr[k];
        }
    }
    acc
}

/// y += a x, element by element.
#[cfg(target_arch = "wasm32")]
fn axpy(y: &mut [f32], a: f32, x: &[f32]) {
    axpy_simd(y, a, x)
}

#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn axpy_simd(y: &mut [f32], a: f32, x: &[f32]) {
    use core::arch::wasm32::{f32x4_add, f32x4_mul, f32x4_splat, v128, v128_load, v128_store};
    assert_eq!(y.len(), x.len());
    let av = f32x4_splat(a);
    let whole = y.len() / 4 * 4;
    for i in (0..whole).step_by(4) {
        // SAFETY: i + 4 <= whole <= len of both slices; unaligned loads and stores are allowed.
        unsafe {
            let yp = y.as_mut_ptr().add(i) as *mut v128;
            v128_store(
                yp,
                f32x4_add(
                    v128_load(yp),
                    f32x4_mul(av, v128_load(x.as_ptr().add(i) as *const v128)),
                ),
            );
        }
    }
    for (yi, &xi) in y[whole..].iter_mut().zip(&x[whole..]) {
        *yi += a * xi;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn axpy(y: &mut [f32], a: f32, x: &[f32]) {
    for (yi, &xi) in y.iter_mut().zip(x) {
        *yi += a * xi;
    }
}
