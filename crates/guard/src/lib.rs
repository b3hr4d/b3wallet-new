//! The B3Wallet guard: reads the person's own words ("send 80 ICP to Dana, she still owes me 90"), marks where the
//! amount, asset, chain, from account, recipient, memo and fee are and what kind of request it is, and checks a
//! proposed transfer against them field by field. The Rust port of candid-profile's shipped guard
//! (`packages/profile-router/guard/`): stage 1 (the round-9 BERT-mini tagger, the comparator and the request rule)
//! and stage 2 (the keyword list cut to the 1% false-flag bar, for texts an approver reads).
//!
//! **The one rule.** The guard only adds flags for a person to read. It never approves, never removes a flag and
//! never outputs a value: the model's answer is a class plus spans of the request, and every verdict is made by
//! deterministic code. "Not checked" is its own answer and is never shown as OK.
//!
//! The model comes from one file, `guard.bin` (candid-profile `guard/canister/export.py`): the W8 tagger's
//! container (b"CPTAGGR1", u32 LE header length, a JSON header, padding to 4 bytes, f32 LE tensors) whose header
//! also carries the release's labels, vocabulary and tokenizer tables. Pure Rust, no host imports.

mod comparator;
mod decode;
mod manip;
pub mod model;
mod request_rule;
mod tokenizer;

pub use comparator::{
    Account, Chain, Check, Context, FIELDS, Overall, Proposal, Token, Verdict, compare,
};
pub use manip::{Flags, Manip, Matched};
pub use request_rule::RequestRule;
pub use tokenizer::{MAX_POSITIONS, MAX_REQUEST_BYTES, Piece, Tokenizer, identifier_kind};

use decode::{argmax, consistent, decode, snap, softmax};
use model::Model;
use serde_json::Value;

pub const MAGIC: &[u8; 8] = b"CPTAGGR1";

/// The guard's answer for one request, as candid-profile's `predict.py` prints it, plus the model's input length
/// (none for a request too long to run).
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub class: String,
    pub spans: Vec<(String, (usize, usize))>,
    pub confidence: f32,
    pub abstain: bool,
    pub positions: Option<usize>,
}

/// Where the parts of a guard file are: (header bytes, offset of the f32 data).
pub fn split_file(head: &[u8]) -> Result<(usize, usize), String> {
    if head.len() < 12 || &head[..8] != MAGIC {
        return Err("not a guard file".into());
    }
    let len = u32::from_le_bytes([head[8], head[9], head[10], head[11]]) as usize;
    Ok((len, (12 + len).div_ceil(4) * 4))
}

/// The header, checked to be this format.
pub fn parse_header(bytes: &[u8]) -> Result<Value, String> {
    let header: Value = serde_json::from_slice(bytes).map_err(|e| format!("header: {e}"))?;
    if header["format"] != "b3wallet-guard" || header["version"] != 1 {
        return Err("not a b3wallet-guard file".into());
    }
    Ok(header)
}

/// How many f32 values follow the header: the end of the last tensor.
pub fn data_values(header: &Value) -> Result<usize, String> {
    let mut end = 0;
    for t in header["tensors"].as_array().ok_or("no tensor table")? {
        let size: Option<u64> = t["shape"]
            .as_array()
            .and_then(|s| s.iter().map(Value::as_u64).product());
        end = end.max(
            t["offset"]
                .as_u64()
                .zip(size)
                .map(|(o, n)| o + n)
                .ok_or("a tensor has no offset or shape")?,
        );
    }
    Ok(end as usize)
}

fn strings(v: &Value, what: &str) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or(format!("{what} is missing"))?
        .iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or(format!("{what} holds a non-string"))
        })
        .collect()
}

pub struct Guard {
    pub model: Model,
    pub tokenizer: Tokenizer,
    pub classes: Vec<String>,
    pub tags: Vec<String>,
    pub threshold: f64,
    pub snap_words: bool,
    /// The sha256 of the release's model.safetensors, from the header.
    pub model_sha256: String,
    pub release: String,
    rule: RequestRule,
    manip: Manip,
}

impl Guard {
    pub fn from_bytes(bytes: &[u8]) -> Result<Guard, String> {
        let (len, start) = split_file(bytes)?;
        let header = parse_header(bytes.get(12..12 + len).ok_or("the header is cut short")?)?;
        let body = bytes.get(start..).ok_or("no data")?;
        if body.len() != 4 * data_values(&header)? {
            return Err("the data is not the size the header gives".into());
        }
        Guard::from_parts(
            &header,
            body.chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        )
    }

    pub fn from_parts(header: &Value, data: Vec<f32>) -> Result<Guard, String> {
        let l = &header["labels"];
        let classes = strings(&l["classes"], "labels.classes")?;
        let tags = strings(&l["tags"], "labels.tags")?;
        if tags.first().map(String::as_str) != Some("O")
            || tags[1..]
                .iter()
                .any(|t| !(t.starts_with("B-") || t.starts_with("I-")))
        {
            return Err("labels.tags is not O, B-*, I-*".into());
        }
        if header["config"]["max_len"].as_u64() != Some(MAX_POSITIONS as u64) {
            return Err("config.max_len is not 128".into());
        }
        let vocab = strings(&header["vocab"], "vocab")?;
        Ok(Guard {
            model: Model::new(header, classes.len(), tags.len(), vocab.len(), data)?,
            tokenizer: Tokenizer::new(&vocab, tokenizer::Tables::from_json(&header["tables"])?)?,
            rule: RequestRule::new(&vocab),
            manip: Manip::default(),
            threshold: l["threshold"]
                .as_f64()
                .ok_or("labels.threshold is missing")?,
            snap_words: l["snap_words"].as_bool().unwrap_or(false),
            classes,
            tags,
            model_sha256: header["model_sha256"].as_str().unwrap_or("").to_owned(),
            release: header["release"].as_str().unwrap_or("").to_owned(),
        })
    }

    /// The answer from the logits (class logits, then the tag logits of every position, [CLS] and [SEP] included):
    /// `tagging.prediction`, then `tagging.guard_json`.
    pub fn answer_from(
        &self,
        text: &[char],
        pieces: &[Piece],
        class_logits: &[f32],
        tag_logits: &[f32],
    ) -> Answer {
        let class_probs = softmax(class_logits);
        let c = argmax(&class_probs);
        let mut confidence = class_probs[c];
        let mut tags = Vec::with_capacity(pieces.len());
        for row in tag_logits
            .chunks_exact(self.tags.len())
            .skip(1)
            .take(pieces.len())
        {
            let p = softmax(row);
            let t = argmax(&p);
            confidence = confidence.min(p[t]);
            tags.push(self.tags[t].as_str());
        }
        let mut spans = decode(pieces, &tags);
        if self.snap_words {
            snap(text, &mut spans);
        }
        let class = self.classes[c].clone();
        let consistent = consistent(&class, &spans, text);
        Answer {
            abstain: !consistent || f64::from(confidence) < self.threshold,
            spans: spans.iter().map(|(k, v)| (k.clone(), v[0])).collect(),
            class,
            confidence,
            positions: Some(pieces.len() + 2),
        }
    }

    /// The guard's answer for a request; Err for one over 280 bytes. A request over 128 positions is not cut (a cut
    /// could shorten an amount or an address): the guard abstains.
    pub fn answer(&self, request: &str) -> Result<Answer, String> {
        if request.len() > MAX_REQUEST_BYTES {
            return Err(format!("a request is at most {MAX_REQUEST_BYTES} bytes"));
        }
        let text: Vec<char> = request.chars().collect();
        let Some((ids, pieces)) = self.tokenizer.encode(&text) else {
            return Ok(Answer {
                class: "not_transfer".into(),
                spans: Vec::new(),
                confidence: 0.0,
                abstain: true,
                positions: None,
            });
        };
        let (class_logits, tag_logits) = self.model.forward(&ids, &vec![0; ids.len()]);
        Ok(self.answer_from(&text, &pieces, &class_logits, &tag_logits))
    }

    /// The request rule's flags: says_not_to, repeats.
    pub fn request_flags(&self, request: &str) -> Vec<&'static str> {
        self.rule.flags(request)
    }

    /// The comparator's verdicts for `proposal`, with the request rule's flags.
    pub fn compare(
        &self,
        answer: &Answer,
        request: &str,
        ctx: &Context,
        proposal: &Proposal,
    ) -> Check {
        let checked = !answer.abstain && answer.class == "one_transfer";
        comparator::compare(
            checked,
            &answer.spans,
            request,
            ctx,
            proposal,
            self.request_flags(request),
        )
    }

    /// Stage 2's flags for a text an approver reads.
    pub fn manip(&self, text: &str) -> Flags {
        self.manip.flag(text)
    }
}
