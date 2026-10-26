//! Decoding, the Rust port of candid-profile's `guard/tagging.py`: tags to spans, spans widened to whole words, the
//! heads-disagree rule, confidence.

use crate::tokenizer::{Piece, is_alnum, is_digit};

/// `tagging.cuts`: position i (between text[i-1] and text[i]) is inside a word or a number.
pub(crate) fn cuts(text: &[char], i: usize) -> bool {
    if i == 0 || i >= text.len() {
        return false;
    }
    let (a, b) = (text[i - 1], text[i]);
    let next = text.get(i + 1).copied();
    let before = if i >= 2 { Some(text[i - 2]) } else { None };
    (is_alnum(a) && is_alnum(b))
        || ((b == '.' || b == ',') && is_digit(a) && next.is_some_and(is_digit))
        || ((a == '.' || a == ',') && is_digit(b) && before.is_some_and(is_digit))
        || (b == '-' && is_alnum(a) && next.is_some_and(is_alnum))
        || (a == '-' && is_alnum(b) && before.is_some_and(is_alnum))
}

/// Every span of every slot, slots in the order they first occur: `tagging.decode`, then `snap` when `snap_words`.
pub(crate) type Spans = Vec<(String, Vec<(usize, usize)>)>;

fn add(spans: &mut Spans, slot: &str, span: (usize, usize)) {
    match spans.iter_mut().find(|(s, _)| s == slot) {
        Some((_, v)) => v.push(span),
        None => spans.push((slot.to_owned(), vec![span])),
    }
}

pub(crate) fn decode(pieces: &[Piece], tags: &[&str]) -> Spans {
    let mut out = Spans::new();
    let mut open: Option<(&str, usize, usize)> = None;
    for (p, &tag) in pieces.iter().zip(tags) {
        let Some((kind, slot)) = tag.split_once('-') else {
            if let Some((s, a, b)) = open.take() {
                add(&mut out, s, (a, b));
            }
            continue;
        };
        match open {
            Some((s, a, _)) if kind != "B" && s == slot => open = Some((s, a, p.end)),
            _ => {
                if let Some((s, a, b)) = open.take() {
                    add(&mut out, s, (a, b));
                }
                open = Some((slot, p.start, p.end));
            }
        }
    }
    if let Some((s, a, b)) = open {
        add(&mut out, s, (a, b));
    }
    out
}

pub(crate) fn snap(text: &[char], spans: &mut Spans) {
    for (_, v) in spans.iter_mut() {
        for (a, b) in v.iter_mut() {
            while cuts(text, *a) {
                *a -= 1;
            }
            while cuts(text, *b) {
                *b += 1;
            }
        }
    }
}

pub(crate) fn consistent(class: &str, spans: &Spans, text: &[char]) -> bool {
    let mut all: Vec<(usize, usize)> = spans.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    all.sort();
    if spans.iter().any(|(_, v)| v.len() > 1)
        || all.iter().any(|&(a, b)| cuts(text, a) || cuts(text, b))
        || all.windows(2).any(|w| w[0].1 > w[1].0)
    {
        return false;
    }
    let has = |s: &str| spans.iter().any(|(k, _)| k == s);
    let core = has("amount") && has("asset") && has("to");
    match class {
        "one_transfer" => core,
        "incomplete" => !core,
        "relative_amount" | "other_currency" => !has("amount"),
        _ => spans.is_empty(),
    }
}

pub(crate) fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| libm::expf(x - max)).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|e| e / sum).collect()
}

/// The first index of the largest value, as argmax does.
pub(crate) fn argmax(v: &[f32]) -> usize {
    (0..v.len()).fold(0, |best, i| if v[i] > v[best] { i } else { best })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_follow_tagging_py() {
        let t: Vec<char> = "Lotte 87.74 INV-2025".chars().collect();
        assert!(cuts(&t, 2) && cuts(&t, 8) && cuts(&t, 15) && cuts(&t, 16));
        assert!(!cuts(&t, 5) && !cuts(&t, 0) && !cuts(&t, t.len()));
    }
}
