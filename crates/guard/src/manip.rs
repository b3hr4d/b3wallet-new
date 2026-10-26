//! Stage 2's flagger for texts an approver reads (a member's label, a transfer's memo, an extension's name and
//! description): the Rust port of candid-profile's `manip_flag.py`, the keyword list cut to the 1% false-flag bar
//! (`guard/manip_rule.py` `flag()` with `release-stage2/trimmed-keywords.json`, 73 of its 96 patterns). No model.
//! It only adds flags for a person to read; no flag is not "approved".
//!
//! Each Python pattern is `(?<![\w])(?:pattern)(?![\w])`, case aside. Rust's `regex` has no look-around, so each
//! one is tried anchored at every start the look-behind allows, as Python's search does, with "followed by a
//! non-word character or the end" in place of the look-ahead: the same accepted matches, in the same order of
//! preference, so the same match. A word character is a letter, a number or "_", as `tokenizer::is_alnum` reads
//! Python's `str.isalnum`.

use crate::tokenizer::is_alnum;
use regex::Regex;

const KEPT: [(&str, &str); 73] = [
    ("urgency", r"asap"),
    ("urgency", r"a\.s\.a\.p"),
    ("urgency", r"immediately"),
    ("urgency", r"right\s+(?:now|away)"),
    ("urgency", r"at\s+once"),
    ("urgency", r"hurry"),
    ("urgency", r"time[-\s]?sensitive"),
    ("urgency", r"last\s+chance"),
    ("urgency", r"before\s+it'?s\s+too\s+late"),
    ("urgency", r"now!+"),
    ("urgency", r"today\s+only"),
    (
        "urgency",
        r"within\s+\d+\s*(?:min(?:ute)?s?|hours?|hrs?|h|m|seconds?|secs?)",
    ),
    ("urgency", r"in\s+\d+\s*(?:min(?:ute)?s?|mins)"),
    ("urgency", r"don'?t\s+wait"),
    ("urgency", r"no\s+time"),
    ("urgency", r"act\s+now"),
    ("urgency", r"frozen"),
    ("urgency", r"suspended"),
    ("urgency", r"at\s+risk"),
    ("urgency", r"or\s+(?:lose|be\s+lost)"),
    ("authority", r"ceo"),
    ("authority", r"cfo"),
    ("authority", r"cto"),
    ("authority", r"coo"),
    ("authority", r"boss"),
    ("authority", r"founder"),
    ("authority", r"director"),
    ("authority", r"root"),
    ("authority", r"superuser"),
    ("authority", r"official(?:ly)?"),
    ("authority", r"security\s+team"),
    ("authority", r"compliance"),
    ("authority", r"it\s+(?:department|dept|team)"),
    ("authority", r"dfinity"),
    ("authority", r"b3wallet"),
    ("authority", r"icp\s+foundation"),
    ("authority", r"foundation"),
    ("authority", r"irs"),
    ("authority", r"government"),
    ("authority", r"regulator"),
    ("authority", r"on\s+behalf\s+of"),
    ("authority", r"by\s+order\s+of"),
    (
        "authority",
        r"per\s+(?:the\s+)?(?:ceo|cfo|owner|boss|management)",
    ),
    ("authority", r"staff"),
    ("authority", r"headquarters"),
    ("authority", r"hq"),
    (
        "approved",
        r"already\s+(?:approved|signed|verified|checked|cleared|ok'?d|confirmed)",
    ),
    ("approved", r"signed\s+off"),
    ("approved", r"sign[-\s]?off"),
    ("approved", r"vetted"),
    ("approved", r"authori[sz]ed"),
    ("approved", r"cleared"),
    ("approved", r"allowlisted"),
    ("approved", r"trusted"),
    ("approved", r"ok'?d"),
    ("approved", r"okayed"),
    ("approved", r"green[-\s]?lit"),
    ("approved", r"confirmed"),
    (
        "approved",
        r"\d+\s*(?:of|/)\s*\d+\s+(?:approvals?|signatures?|sign[-\s]?offs?|owners?|signers?)",
    ),
    ("approved", r"no\s+review\s+needed"),
    ("approved", r"reviewed"),
    ("instruction", r"accept"),
    ("instruction", r"sign\s+(?:it|this|now|here)"),
    ("instruction", r"click"),
    ("instruction", r"tap"),
    ("instruction", r"dismiss"),
    ("instruction", r"disregard"),
    (
        "instruction",
        r"(?:do\s+not|don'?t|no\s+need\s+to)\s+(?:check|verify|review|look|read|reject|worry)",
    ),
    ("instruction", r"just\s+(?:approve|sign|click|confirm)"),
    ("instruction", r"trust\s+me"),
    ("instruction", r"no\s+questions"),
    ("instruction", r"without\s+(?:checking|reviewing)"),
    ("instruction", r"please\s+(?:approve|sign|confirm|accept)"),
];

/// Words that matched a cue, `[start, end)` in code points of the text, and the text itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matched {
    pub cue: &'static str,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flags {
    /// The cues flagged, sorted.
    pub flags: Vec<&'static str>,
    /// Every match, by start, end, then cue.
    pub matched: Vec<Matched>,
}

pub struct Manip {
    patterns: Vec<(&'static str, Regex)>,
}

fn word(c: char) -> bool {
    is_alnum(c) || c == '_'
}

impl Default for Manip {
    fn default() -> Manip {
        let patterns = KEPT
            .iter()
            .map(|(cue, p)| {
                (
                    *cue,
                    Regex::new(&format!(r"(?i)\A((?:{p}))(?:[^\p{{L}}\p{{N}}_]|\z)"))
                        .expect("a cue pattern compiles"),
                )
            })
            .collect();
        Manip { patterns }
    }
}

impl Manip {
    pub fn flag(&self, text: &str) -> Flags {
        let mut found: Vec<(usize, usize, &'static str)> = Vec::new();
        for (cue, re) in &self.patterns {
            let mut next = 0; // finditer goes on from the end of the last match
            let mut prev: Option<char> = None;
            for (s, c) in text.char_indices() {
                if s >= next
                    && !prev.is_some_and(word)
                    && let Some(m) = re.captures(&text[s..]).and_then(|c| c.get(1))
                {
                    next = s + m.end();
                    found.push((s, next, cue));
                }
                prev = Some(c);
            }
        }
        let at = |b: usize| text[..b].chars().count();
        let mut spans: Vec<(usize, usize, &'static str)> = found
            .into_iter()
            .map(|(a, b, c)| (at(a), at(b), c))
            .collect();
        spans.sort();
        spans.dedup();
        let mut flags: Vec<&'static str> = spans.iter().map(|s| s.2).collect();
        flags.sort();
        flags.dedup();
        let chars: Vec<char> = text.chars().collect();
        Flags {
            flags,
            matched: spans
                .into_iter()
                .map(|(a, b, cue)| Matched {
                    cue,
                    start: a,
                    end: b,
                    text: chars[a..b].iter().collect(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_only() {
        let m = Manip::default();
        assert_eq!(m.flag("Claude on my Mac").flags, Vec::<&str>::new());
        assert_eq!(
            m.flag("approved by CEO, sign now").flags,
            vec!["authority", "instruction"]
        );
        assert_eq!(m.flag("CEOs and ceo_x").flags, Vec::<&str>::new());
        assert_eq!(m.flag("now!!! pay").matched[0].text, "now!!!");
    }
}
