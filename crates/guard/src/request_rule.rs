//! The request rule, the Rust port of candid-profile's `guard/request_rule.py` (round 9): deterministic code over
//! the request's own words that adds flags, whatever the guard read. A flag can never be removed.
//!
//! - "says_not_to": a negation ("don't", "never", "stop", "cancel", "block", "decline" ...) followed within three
//!   words by a sending word ("send", "pay", "transfer", "payout" ...), or a sending word followed within five
//!   words by "on hold". A comma, a colon, a conjunction or "forget", "hesitate", "wait", "delay" in between ends
//!   the negation ("don't forget to send" asks for the transfer).
//! - "repeats": "twice", "<n> times", "every <period>", "weekly", "recurring" and the like.
//!
//! Typos one edit from a cue word ("sedn", "evrey") count as that word, unless they are English words (whole
//! words of the model's vocabulary). The patterns are Python's, which Rust's `regex` reads the same way here: no
//! look-around, leftmost-first alternation, lazy repetition.

use regex::{Captures, Regex};
use std::collections::HashSet;

const NEG: &str = r"don['’]?t|do\s+not|does\s+not|doesn['’]?t|never|should\s+not|shouldn['’]?t|must\s+not|mustn['’]?t|won['’]?t|will\s+not|wouldn['’]?t|would\s+not|not|no\s+need\s+to|no\s+longer|stop|quit|refrain\s+from|avoid|hold\s+off(?:\s+on)?|cancel|undo|revoke|reverse|block|freeze|halt|pause|suspend|hold|put\s+on\s+hold|delete|remove|refuse|reject|decline|deny|abort|call\s+off|scrap";
const SEND: &str = r"send|sends|sending|sent|transfer|transfers|transferring|transferred|pay|pays|paying|paid|payment|payments|move|moves|moving|give|gives|giving|wire|wiring|forward|forwarding|tip|tipping|remit|remitting|refund|refunding|reimburse|reimbursing|deliver|delivering|withdraw|withdrawing|fund|funding|route|routing|transaction|transactions|top\s+up|topping\s+up|payout|payouts|disbursement|disbursements|remittance|remittances|dispatch|dispatching";
const PERIOD: &str = r"day|days|week|weeks|month|months|year|years|hour|hours|morning|evening|night|weekend|monday|tuesday|wednesday|thursday|friday|saturday|sunday|month\s+end|quarter|fortnight";
const COUNT: &str = r"\d+|two|three|four|five|six|seven|eight|nine|ten|several|multiple|many|a\s+few|a\s+couple\s+of";
const CUES: &[&str] = &[
    "send",
    "sending",
    "transfer",
    "transferring",
    "payment",
    "paying",
    "move",
    "moving",
    "give",
    "giving",
    "wire",
    "forward",
    "remit",
    "refund",
    "deliver",
    "withdraw",
    "dont",
    "never",
    "stop",
    "cancel",
    "shouldnt",
    "hold",
    "every",
    "twice",
    "times",
    "weekly",
    "monthly",
    "daily",
    "month",
    "months",
    "week",
    "weeks",
    "repeat",
    "forget",
    "hesitate",
    "wait",
    "delay",
    "block",
    "freeze",
    "pause",
    "payout",
    "delete",
    "remove",
    "refuse",
    "reject",
    "decline",
    "abort",
    "dispatch",
];

/// The rule, with the English words of the model's vocabulary (whole ASCII words of letters only).
pub struct RequestRule {
    english: HashSet<String>,
    negated_send: Regex,
    send_on_hold: Regex,
    gap_ends: Regex,
    repeats: Regex,
    word: Regex,
}

/// True when a and b (ASCII) differ by at most one dropped, added or changed letter, or one swap of neighbours.
fn one_edit(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a == b {
        return true;
    }
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    if a.len() == b.len() {
        let diff: Vec<usize> = (0..a.len()).filter(|&k| a[k] != b[k]).collect();
        return diff.len() == 1
            || (diff.len() == 2
                && diff[1] == diff[0] + 1
                && a[diff[0]] == b[diff[1]]
                && a[diff[1]] == b[diff[0]]);
    }
    let (a, b) = if a.len() > b.len() { (b, a) } else { (a, b) };
    let k = (0..a.len()).find(|&k| a[k] != b[k]).unwrap_or(a.len());
    a[k..] == b[k + 1..]
}

impl RequestRule {
    pub fn new(vocab: &[String]) -> RequestRule {
        let re = |p: String| Regex::new(&p).expect("a request-rule pattern compiles");
        RequestRule {
            english: vocab
                .iter()
                .filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphabetic()))
                .cloned()
                .collect(),
            negated_send: re(format!(
                r"(?i)\b(?:{NEG})\b((?:\s+[^\s,;:!?]+){{0,3}}?)\s+(?:{SEND})\b"
            )),
            send_on_hold: re(format!(
                r"(?i)\b(?:{SEND})\b((?:\s+[^\s,;:!?]+){{0,5}}?)\s+on\s+hold\b"
            )),
            gap_ends: re(
                r"(?i)\b(?:forget|forgetting|hesitate|wait|delay|but|and|then|so|or|instead)\b"
                    .into(),
            ),
            repeats: re(format!(
                r"(?i)\b(?:twice|thrice|(?:{COUNT})\s+times|once\s+(?:a|an|per|every)\s+(?:{PERIOD})|(?:every|each|per)\s+(?:other\s+|single\s+|(?:{COUNT})\s+)?(?:{PERIOD})|daily|weekly|monthly|yearly|annually|fortnightly|biweekly|hourly|nightly|recurring|repeatedly|repeat|on\s+repeat)\b"
            )),
            word: re(r"[A-Za-z]+(?:['’][A-Za-z]+)?".into()),
        }
    }

    fn read_typos(&self, request: &str) -> String {
        self.word
            .replace_all(request, |m: &Captures| {
                let word = &m[0];
                let low = word.to_ascii_lowercase().replace(['\'', '’'], "");
                if low.len() < 3 || self.english.contains(&low) {
                    return word.to_owned();
                }
                CUES.iter()
                    .find(|c| one_edit(&low, c))
                    .map_or_else(|| word.to_owned(), |c| (*c).to_owned())
            })
            .into_owned()
    }

    fn says_not_to(&self, text: &str) -> bool {
        let open = |r: &Regex| {
            r.captures_iter(text)
                .any(|c| !self.gap_ends.is_match(&c[1]))
        };
        open(&self.negated_send) || open(&self.send_on_hold)
    }

    /// The flags the request's own words raise, in a fixed order.
    pub fn flags(&self, request: &str) -> Vec<&'static str> {
        let text = self.read_typos(request);
        let mut out = Vec::new();
        if self.says_not_to(&text) {
            out.push("says_not_to");
        }
        if self.repeats.is_match(&text) {
            out.push("repeats");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_edit_matches_python() {
        assert!(one_edit("sedn", "send") && one_edit("evrey", "every") && one_edit("sned", "send"));
        assert!(
            one_edit("tranfer", "transfer") && !one_edit("end", "send ") && !one_edit("abc", "xyz")
        );
    }
}
