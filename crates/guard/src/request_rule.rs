//! The request rule, the Rust port of candid-profile's `guard/request_rule.py` (round 9, with candid-profile PR #95
//! of 2026-11-02): deterministic code over the request's own words that adds flags, whatever the guard read. A flag
//! can never be removed.
//!
//! - "says_not_to": a negation ("don't", "never", "stop", "cancel", "block", "decline" ...) followed within three
//!   words by a sending word ("send", "pay", "transfer", "payout" ...), or a sending word followed within five
//!   words by "on hold". A comma, a colon, a conjunction or "forget", "hesitate", "wait", "delay" in between ends
//!   the negation ("don't forget to send" asks for the transfer). "never" before a past sending word, with
//!   "before" closing the clause within four more words, tells of the past ("I have never sent Bitcoin before.
//!   Please send …"); "It must never be sent before Friday" still says not to.
//! - "repeats": "twice", "<n> times", "every <period>", "weekly", "recurring" and the like. The wallet's own
//!   account labels are read as a neutral word where they stand as an account (after "from", "to", "into", "onto",
//!   "my", "our", or before "account", "acct", "wallet", "subaccount"), so an account named "Daily" raises no flag
//!   in "send 3 ICP from my Daily account", and "send 3 ICP to Bob daily" still repeats. "says_not_to" reads the
//!   labels as typed: a label's word can be a real refusal's only sending word ("you must not credit my Payments 4
//!   sub-account").
//!
//! Typos one edit from a cue word ("sedn", "evrey") count as that word, unless they are English words (whole
//! words of the model's vocabulary). The patterns are Python's, which Rust's `regex` reads the same way here:
//! leftmost-first alternation, lazy repetition. It has no look-around, so Python's is consumed instead where only
//! a match's start is read ("never … before"), and checked by code where text is replaced (the labels).

use regex::{Captures, Regex};
use std::collections::HashSet;

const NEG: &str = r"don['’]?t|do\s+not|does\s+not|doesn['’]?t|never|should\s+not|shouldn['’]?t|must\s+not|mustn['’]?t|won['’]?t|will\s+not|wouldn['’]?t|would\s+not|not|no\s+need\s+to|no\s+longer|stop|quit|refrain\s+from|avoid|hold\s+off(?:\s+on)?|cancel|undo|revoke|reverse|block|freeze|halt|pause|suspend|hold|put\s+on\s+hold|delete|remove|refuse|reject|decline|deny|abort|call\s+off|scrap";
const SEND: &str = r"send|sends|sending|sent|transfer|transfers|transferring|transferred|pay|pays|paying|paid|payment|payments|move|moves|moving|give|gives|giving|wire|wiring|forward|forwarding|tip|tipping|remit|remitting|refund|refunding|reimburse|reimbursing|deliver|delivering|withdraw|withdrawing|fund|funding|route|routing|transaction|transactions|top\s+up|topping\s+up|payout|payouts|disbursement|disbursements|remittance|remittances|dispatch|dispatching";
const PERIOD: &str = r"day|days|week|weeks|month|months|year|years|hour|hours|morning|evening|night|weekend|monday|tuesday|wednesday|thursday|friday|saturday|sunday|month\s+end|quarter|fortnight";
const COUNT: &str = r"\d+|two|three|four|five|six|seven|eight|nine|ten|several|multiple|many|a\s+few|a\s+couple\s+of";
const PAST_SEND: &str = r"sent|transferred|paid|moved|gave|given|wired|forwarded|tipped|remitted|refunded|reimbursed|delivered|withdrew|withdrawn|funded|routed|dispatched|topped\s+up";
const ACCOUNT_BEFORE: &str = r"from|to|into|onto|my|our";
const ACCOUNT_AFTER: &str = r"account|accounts|acct|wallet|subaccount";
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
    never_before: Regex,
    account_before: Regex,
    account_after: Regex,
    repeats: Regex,
    word: Regex,
}

// shortcut: Python's `\w` by char::is_alphanumeric, and its strip/split/`\s` by Rust's whitespace. They differ only on
// a few characters (combining marks, U+001C to U+001F), where a label then stays as typed: a flag kept, never lost.
fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Python's `re.sub` for a pattern the `regex` crate cannot spell (look-around): at each match of `re`, `at` gives
/// the end of the text to replace and what replaces it, or refuses, and the search goes on one character later, as
/// Python's engine does. `re` never matches empty text.
fn sub_where(re: &Regex, text: &str, at: impl Fn(&Captures) -> Option<(usize, String)>) -> String {
    let (mut out, mut last, mut from) = (String::new(), 0, 0);
    while let Some(c) = re.captures_at(text, from) {
        let start = c.get(0).expect("the whole match").start();
        if let Some((end, with)) = at(&c) {
            out.push_str(&text[last..start]);
            out.push_str(&with);
            (last, from) = (end, end);
        } else {
            from = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out + &text[last..]
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
            // Python's look-ahead after "before" is consumed: only the match's start is read.
            never_before: re(format!(
                r"(?i)\bnever\s+(?:{PAST_SEND})\b(?:\s+[^\s,;:!?.]+){{0,4}}?\s+before\b(?:\s*(?:$|[,;:!?.)])|\s+(?:so|and|but|or)\b)"
            )),
            account_before: re(format!(r"(?i)\b({ACCOUNT_BEFORE})(\s+)")),
            account_after: re(format!(r"(?i)\A\s+(?:{ACCOUNT_AFTER})\b")),
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

    /// The request with each account label, where it stands as an account, as one neutral word per word of the label
    /// (so the gaps the rules count stay the same). Longest labels first; case-insensitive.
    fn mask_labels(&self, request: &str, labels: &[&str]) -> String {
        let mut labels: Vec<&str> = labels
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        labels.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
        labels.dedup();
        let (low, mut text) = (request.to_lowercase(), request.to_owned());
        for label in labels {
            let words: Vec<&str> = label.split_whitespace().collect();
            // A label's pattern takes about 80 µs to build (natively; a 64-label wallet would add a sixth of the
            // model's run to every check), so only a label whose words are all in the request is built. A label
            // skipped here or too large to build stays as typed: a flag kept, never lost.
            if !words.iter().all(|w| low.contains(&w.to_lowercase())) {
                continue;
            }
            let body = words
                .iter()
                .map(|w| regex::escape(w))
                .collect::<Vec<_>>()
                .join(r"\s+");
            let Ok(label_at) = Regex::new(&format!(r"(?i){body}")) else {
                continue;
            };
            let neutral = vec!["Vault"; words.len()].join(" ");
            // "from Daily": an account word and spaces, then the label, with no word character after it.
            let t = text;
            text = sub_where(&self.account_before, &t, |c| {
                let end = c.get(0)?.end();
                let m = label_at
                    .find_at(&t, end)
                    .filter(|m| m.start() == end && !is_word(t[m.end()..].chars().next()))?;
                Some((m.end(), format!("{}{}{neutral}", &c[1], &c[2])))
            });
            // "Daily account": the label, with no word character before it, then spaces and an account word.
            let t = text;
            text = sub_where(&label_at, &t, |c| {
                let m = c.get(0)?;
                (!is_word(t[..m.start()].chars().next_back())
                    && self.account_after.is_match(&t[m.end()..]))
                .then(|| (m.end(), neutral.clone()))
            });
        }
        text
    }

    fn says_not_to(&self, text: &str) -> bool {
        let past: HashSet<usize> = self
            .never_before
            .find_iter(text)
            .map(|m| m.start())
            .collect();
        let open = |c: &Captures| !self.gap_ends.is_match(&c[1]);
        self.negated_send
            .captures_iter(text)
            .any(|c| !past.contains(&c.get(0).expect("the whole match").start()) && open(&c))
            || self.send_on_hold.captures_iter(text).any(|c| open(&c))
    }

    /// The flags the request's own words raise, in a fixed order. `labels`: the wallet's own account labels, read
    /// as a neutral word where they stand as an account (for "repeats" only).
    pub fn flags(&self, request: &str, labels: &[&str]) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.says_not_to(&self.read_typos(request)) {
            out.push("says_not_to");
        }
        if self
            .repeats
            .is_match(&self.read_typos(&self.mask_labels(request, labels)))
        {
            out.push("repeats");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> RequestRule {
        let vocab: Vec<String> = include_str!("../tests/data/vocab.txt")
            .split('\n')
            .map(str::to_owned)
            .collect();
        RequestRule::new(&vocab)
    }

    // candid-profile tests/test_request_rule.py (2026-11-02), case for case.
    const LABELS: &[&str] = &[
        "Daily",
        "BTC Savings",
        "Hold",
        "Weekly fund",
        "Payments 4",
        "New moving",
    ];

    #[test]
    fn never_before_tells_of_the_past() {
        let rule = rule();
        for text in [
            "Hello, I have never sent Bitcoin before. Please send 0.0008 BTC to Rahul, be careful!",
            "I've never paid anyone in crypto before, so send 5 ICP to Dana slowly",
            "never sent ICP before",
        ] {
            assert!(rule.flags(text, &[]).is_empty(), "{text}");
        }
    }

    #[test]
    fn account_labels_raise_no_flag_where_they_stand_as_an_account() {
        let rule = rule();
        for text in [
            "send 3 ICP from my Daily account to Dana",
            "ICP 88 → Jürgen, fee 0.0001, from Daily",
            "send Yurgen ten ICP from the daily account period",
            "Could you send 15 ckUSDC to Mr. Nakamura from my Daily account?",
            "move 2 ICP into Hold",
            "send 5 ICP from our Weekly fund to Dana",
            "send 0.1 BTC from BTC Savings to bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
        ] {
            assert!(rule.flags(text, LABELS).is_empty(), "{text}");
        }
        // without the wallet's labels the rule reads the words as before
        assert_eq!(
            rule.flags("send 3 ICP from my Daily account to Dana", &[]),
            ["repeats"]
        );
    }

    #[test]
    fn labels_never_hide_a_flag_the_words_raise_elsewhere() {
        let rule = rule();
        for (text, flags) in [
            ("send 3 ICP to Bob daily", "repeats"),
            ("send 3 ICP from Daily to Bob every week", "repeats"),
            ("send 2 ICP to Dana daily from my Daily account", "repeats"),
            ("don't send anything from my Daily account", "says_not_to"),
            ("hold the payment to Dana", "says_not_to"),
            ("put my payment from Hold to Dana on hold", "says_not_to"),
            ("It must never be sent before Friday", "says_not_to"),
            ("never send it before Friday", "says_not_to"),
            ("I never sent it", "says_not_to"),
            (
                "I've never sent Bitcoin before, and don't send any now",
                "says_not_to",
            ),
            // a label's word as the only sending word of a real refusal (both from the generator's training data)
            (
                "you must not credit my Payments 4 sub-account with 4 eth",
                "says_not_to",
            ),
            (
                "Can we freeze my New moving pot for seven days?",
                "says_not_to",
            ),
        ] {
            assert_eq!(rule.flags(text, LABELS), [flags], "{text}");
        }
    }

    #[test]
    fn one_edit_matches_python() {
        assert!(one_edit("sedn", "send") && one_edit("evrey", "every") && one_edit("sned", "send"));
        assert!(
            one_edit("tranfer", "transfer") && !one_edit("end", "send ") && !one_edit("abc", "xyz")
        );
    }
}
