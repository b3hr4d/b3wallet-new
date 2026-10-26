//! The guard's tokenizer, the Rust port of candid-profile's `guard/tokenizer.py`, rule for rule from the release's
//! `vocab.txt` and `tokenizer-tables.json` (both in the guard file's header): BERT uncased WordPiece with one
//! placeholder piece per address, every piece keeping the code points of the request it covers.

use serde_json::Value;
use std::collections::HashMap;
use unicode_general_category::{GeneralCategory as G, get_general_category};

pub const MAX_POSITIONS: usize = 128;
pub const MAX_REQUEST_BYTES: usize = 280;
const MAX_WORD_CHARS: usize = 100;
const LEADING: &str = "\"'([{<`“‘«";
const TRAILING: &str = ".,;:!?\"')]}>`”’»";
const PLACEHOLDERS: [&str; 5] = [
    "[unused1]",
    "[unused2]",
    "[unused3]",
    "[unused4]",
    "[unused5]",
];

/// `tokenizer-tables.json`: code point ranges and the fold of each character that folds.
pub struct Tables {
    whitespace: Vec<(u32, u32)>,
    drop: Vec<(u32, u32)>,
    split: Vec<(u32, u32)>,
    split_from: u32,
    fold: HashMap<char, Vec<char>>,
}

fn ranges(v: &Value, what: &str) -> Result<Vec<(u32, u32)>, String> {
    let bad = || format!("tables.{what} is not a list of [lo, hi]");
    v.as_array()
        .ok_or_else(bad)?
        .iter()
        .map(|r| Some((r.get(0)?.as_u64()? as u32, r.get(1)?.as_u64()? as u32)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(bad)
}

fn within(cp: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|&(lo, hi)| lo <= cp && cp <= hi)
}

impl Tables {
    pub fn from_json(t: &Value) -> Result<Tables, String> {
        let mut fold = HashMap::new();
        for (k, v) in t["fold"].as_object().ok_or("tables.fold is missing")? {
            let c = u32::from_str_radix(k, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or(format!("tables.fold: bad key {k}"))?;
            fold.insert(
                c,
                v.as_str()
                    .ok_or("tables.fold holds a non-string")?
                    .chars()
                    .collect(),
            );
        }
        let split = ranges(&t["split"], "split")?;
        if split.windows(2).any(|w| w[0].0 > w[1].0) {
            return Err("tables.split is not sorted".into());
        }
        Ok(Tables {
            whitespace: ranges(&t["whitespace"], "whitespace")?,
            drop: ranges(&t["drop"], "drop")?,
            split,
            split_from: t["split_from"]
                .as_u64()
                .ok_or("tables.split_from is missing")? as u32,
            fold,
        })
    }

    fn is_whitespace(&self, c: char) -> bool {
        within(c as u32, &self.whitespace)
    }

    fn is_split(&self, c: char) -> bool {
        let cp = c as u32;
        let k = self.split.partition_point(|&(lo, _)| lo <= cp);
        cp >= self.split_from || (k > 0 && cp <= self.split[k - 1].1)
    }
}

/// A word piece: its vocabulary id and the code points of the request it covers, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub id: u32,
    pub start: usize,
    pub end: usize,
}

pub struct Tokenizer {
    ids: HashMap<String, u32>,
    tables: Tables,
    unk: u32,
    cls: u32,
    sep: u32,
    placeholders: [u32; 5],
}

/// `tokenizer.identifier_kind`: the placeholder (0 for [unused1] .. 4 for [unused5]) for a stripped,
/// whitespace-free core, or None.
pub fn identifier_kind(core: &[char]) -> Option<usize> {
    if !core.iter().all(char::is_ascii) {
        return None;
    }
    let s: String = core.iter().collect();
    let hex_after_0x = |min: usize, exact: bool| {
        let rest = &s.as_bytes()[2.min(s.len())..];
        (s.starts_with("0x") || s.starts_with("0X"))
            && if exact {
                rest.len() == min
            } else {
                rest.len() >= min
            }
            && rest.iter().all(u8::is_ascii_hexdigit)
    };
    let b = s.as_bytes();
    let bech32 = |c: &u8| b"023456789acdefghjklmnpqrstuvwxyz".contains(&c.to_ascii_lowercase());
    if hex_after_0x(40, true) {
        return Some(3);
    }
    if b.len() >= 9
        && ((b[..2].eq_ignore_ascii_case(b"bc")) || b[..2].eq_ignore_ascii_case(b"tb"))
        && b[2] == b'1'
        && b.len() - 3 <= 87
        && b[3..].iter().all(bech32)
    {
        return Some(2);
    }
    if b.len() >= 20
        && b.iter()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-'))
        && s.matches('-').count() >= 4
    {
        return Some(if s.contains('.') { 1 } else { 0 });
    }
    if hex_after_0x(8, false) {
        return Some(4);
    }
    if b.len() >= 25
        && b.iter().all(u8::is_ascii_alphanumeric)
        && b.iter().any(u8::is_ascii_digit)
        && b.iter().any(u8::is_ascii_alphabetic)
    {
        return Some(4);
    }
    None
}

impl Tokenizer {
    pub fn new(vocab: &[String], tables: Tables) -> Result<Tokenizer, String> {
        let ids: HashMap<String, u32> = vocab
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i as u32))
            .collect();
        let id = |p: &str| {
            ids.get(p)
                .copied()
                .ok_or(format!("{p} is not in the vocabulary"))
        };
        let mut placeholders = [0; 5];
        for (slot, p) in placeholders.iter_mut().zip(PLACEHOLDERS) {
            *slot = id(p)?;
        }
        Ok(Tokenizer {
            unk: id("[UNK]")?,
            cls: id("[CLS]")?,
            sep: id("[SEP]")?,
            placeholders,
            ids,
            tables,
        })
    }

    /// (start, end, placeholder) of each identifier in `text` (rule 1).
    fn identifiers(&self, text: &[char]) -> Vec<(usize, usize, usize)> {
        let ws = |c: char| self.tables.is_whitespace(c);
        let (mut out, mut i, n) = (Vec::new(), 0, text.len());
        while i < n {
            if ws(text[i]) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < n && !ws(text[j]) {
                j += 1;
            }
            let (mut a, mut b) = (i, j);
            while a < b && LEADING.contains(text[a]) {
                a += 1;
            }
            while b > a && TRAILING.contains(text[b - 1]) {
                b -= 1;
            }
            if let Some(kind) = (b > a).then(|| identifier_kind(&text[a..b])).flatten() {
                out.push((a, b, kind));
            }
            i = j;
        }
        out
    }

    /// The words of `text[a..b]` (rule 2), each with the request index of every character.
    fn words(&self, text: &[char], a: usize, b: usize) -> Vec<(Vec<char>, Vec<usize>)> {
        let t = &self.tables;
        let (mut out, mut chars, mut src) = (Vec::new(), Vec::new(), Vec::new());
        for (i, &c) in text.iter().enumerate().take(b).skip(a) {
            if t.is_whitespace(c) {
                if !chars.is_empty() {
                    out.push((std::mem::take(&mut chars), std::mem::take(&mut src)));
                }
            } else if within(c as u32, &t.drop) {
                continue;
            } else if t.is_split(c) {
                if !chars.is_empty() {
                    out.push((std::mem::take(&mut chars), std::mem::take(&mut src)));
                }
                out.push((vec![c], vec![i]));
            } else {
                match t.fold.get(&c) {
                    Some(f) => {
                        chars.extend(f);
                        src.extend(std::iter::repeat_n(i, f.len()));
                    }
                    None => {
                        chars.push(c);
                        src.push(i);
                    }
                }
            }
        }
        if !chars.is_empty() {
            out.push((chars, src));
        }
        out
    }

    /// Rule 3: greedy longest-match pieces of one word, or one [UNK] for the whole word.
    fn wordpiece(&self, word: &[char], src: &[usize], out: &mut Vec<Piece>) {
        let unk = Piece {
            id: self.unk,
            start: src[0],
            end: src[src.len() - 1] + 1,
        };
        if word.len() > MAX_WORD_CHARS {
            out.push(unk);
            return;
        }
        let mut found = Vec::new();
        let mut start = 0;
        while start < word.len() {
            let mut end = word.len();
            let mut piece = None;
            while start < end {
                let mut candidate = if start == 0 {
                    String::new()
                } else {
                    "##".to_owned()
                };
                candidate.extend(&word[start..end]);
                if let Some(&id) = self.ids.get(&candidate) {
                    piece = Some(id);
                    break;
                }
                end -= 1;
            }
            let Some(id) = piece else {
                out.push(unk);
                return;
            };
            found.push(Piece {
                id,
                start: src[start],
                end: src[end - 1] + 1,
            });
            start = end;
        }
        out.extend(found);
    }

    pub fn tokenize(&self, text: &[char]) -> Vec<Piece> {
        let mut pieces = Vec::new();
        let mut cursor = 0;
        for (start, end, kind) in self.identifiers(text) {
            for (word, src) in self.words(text, cursor, start) {
                self.wordpiece(&word, &src, &mut pieces);
            }
            pieces.push(Piece {
                id: self.placeholders[kind],
                start,
                end,
            });
            cursor = end;
        }
        for (word, src) in self.words(text, cursor, text.len()) {
            self.wordpiece(&word, &src, &mut pieces);
        }
        pieces
    }

    /// [CLS] pieces [SEP] and the pieces, or None when that is more than MAX_POSITIONS positions (the request is
    /// never cut: a cut could shorten an amount or an address). Token types are all 0.
    pub fn encode(&self, text: &[char]) -> Option<(Vec<u32>, Vec<Piece>)> {
        let pieces = self.tokenize(text);
        if pieces.len() + 2 > MAX_POSITIONS {
            return None;
        }
        let ids = std::iter::once(self.cls)
            .chain(pieces.iter().map(|p| p.id))
            .chain([self.sep])
            .collect();
        Some((ids, pieces))
    }
}

// shortcut: Python's str.isalpha / isdigit / isalnum by general category only (L*, Nd, L* or N*); Python also counts
// digit-type No characters such as "²" as digits. Only spans next to such characters can differ; upgrade with a
// Numeric_Type table if a parity case ever shows one.
pub(crate) fn is_alpha(c: char) -> bool {
    matches!(
        get_general_category(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
    )
}

pub(crate) fn is_digit(c: char) -> bool {
    get_general_category(c) == G::DecimalNumber
}

pub(crate) fn is_alnum(c: char) -> bool {
    is_alpha(c)
        || matches!(
            get_general_category(c),
            G::DecimalNumber | G::LetterNumber | G::OtherNumber
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_kinds() {
        let kind = |s: &str| identifier_kind(&s.chars().collect::<Vec<_>>());
        assert_eq!(kind("0x52908400098527886E0F7030069857D2E4169EE7"), Some(3));
        assert_eq!(kind("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"), Some(2));
        assert_eq!(
            kind("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"),
            Some(4)
        ); // regtest: "another identifier"
        assert_eq!(kind("rrkah-fqaaa-aaaaa-aaaaq-cai"), Some(0));
        assert_eq!(kind("rrkah-fqaaa-aaaaa-aaaaq-cai-abc.1"), Some(1));
        assert_eq!(kind("0xdeadbeef"), Some(4));
        assert_eq!(kind("Main"), None);
    }
}
