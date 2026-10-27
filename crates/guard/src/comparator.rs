//! The comparator, the Rust port of candid-profile's `guard/comparator.py`: deterministic code that checks a
//! proposed transfer against the person's words. The model only says where the words are; every verdict is made
//! here, field by field: `Same`, `Differs` or `NotChecked`, and "not checked" is never shown as OK.
//!
//! - abstain, or a class other than one_transfer: every field is not checked.
//! - amount and fee: the span must be digits with at most one "."; it is compared exactly with amount /
//!   10^decimals (no floats, no rounding). More fractional digits than the asset has, even zeros, differs.
//!   Anything else, or no span: not checked. A fee span against a proposal without a fee: not checked.
//! - asset and chain: the span, case aside, against fixed aliases and the wallet's token symbols. Another known
//!   asset or chain differs; a word that names none is not checked, so an unknown word is never a flag.
//! - to: an address must equal the proposal's `to` (an EVM address case aside); one own account's label means its
//!   address; anything else, such as a person's name, is not checked. from: an account's label or address.
//! - memo: the span's UTF-8 bytes against the proposal's memo; a memo the request never mentions differs.
//! - request flags (`request_rule.rs`) make the overall verdict "differs", also when the guard abstained.

use crate::tokenizer::identifier_kind;
use unicode_general_category::{GeneralCategory, get_general_category};

pub const FIELDS: [&str; 7] = ["amount", "asset", "chain", "from", "to", "memo", "fee"];
const CORE: [&str; 3] = ["amount", "asset", "to"];

/// What an account holds or a transfer moves. Bitcoin carries its network's name ("Mainnet", "Testnet",
/// "Regtest"), Icrc its ledger's principal as text, Evm its EIP-155 chain id (none for an EVM account, which holds
/// the native token of every chain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chain {
    Icp,
    Icrc(String),
    Bitcoin(String),
    Evm(Option<u64>),
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: u32,
    pub label: String,
    pub address: String,
    pub chain: Chain,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub ledger: String,
    pub symbol: String,
    pub decimals: u32,
}

/// The wallet as the comparator reads it.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub accounts: Vec<Account>,
    pub tokens: Vec<Token>,
}

/// A proposed transfer: amounts and fees in base units as decimal digits, the memo's bytes (empty: none).
#[derive(Clone, Debug)]
pub struct Proposal {
    pub from: u32,
    pub asset: Option<Chain>,
    pub to: String,
    pub amount: String,
    pub fee: Option<String>,
    pub memo: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Same,
    Differs,
    NotChecked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overall {
    Differs,
    Matches,
    Partial,
}

/// One proposal's verdicts: per field in `FIELDS` order, the request's flags, and overall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub fields: Vec<(&'static str, Verdict)>,
    pub flags: Vec<&'static str>,
    pub overall: Overall,
    pub unchecked: Vec<&'static str>,
}

const ASSET_ALIASES: [(&str, &[&str]); 3] = [
    ("icp", &["icp"]),
    ("btc", &["btc", "bitcoin"]),
    ("eth", &["eth", "ether", "ethereum"]),
];
const CHAIN_ALIASES: [(&str, &[&str]); 9] = [
    ("icp", &["icp", "internet computer"]),
    (
        "btc:Mainnet",
        &["bitcoin", "btc", "mainnet", "bitcoin mainnet"],
    ),
    ("btc:Testnet", &["testnet", "bitcoin testnet"]),
    ("btc:Regtest", &["regtest", "bitcoin regtest"]),
    ("evm:1", &["ethereum", "ethereum mainnet", "mainnet", "eth"]),
    ("evm:11155111", &["sepolia"]),
    ("evm:42161", &["arbitrum", "arbitrum one"]),
    ("evm:8453", &["base"]),
    ("evm:10", &["optimism", "op mainnet"]),
];

// shortcut: Python's casefold, \s and strip by to_lowercase and char::is_whitespace. They differ only on a few
// characters ("ß" folds to "ss"; U+001C to U+001F count as space in Python), where a label or a symbol then
// fails to match and the field is not checked: a lost check, never a wrong "same".
fn fold(text: &str) -> String {
    text.to_lowercase()
}

fn norm(text: &str) -> String {
    let out = fold(text).split_whitespace().collect::<Vec<_>>().join(" ");
    match out.strip_prefix("the ") {
        Some(rest) => rest.to_owned(),
        None => out,
    }
}

/// A decimal digit's value: every Unicode decimal digit, as Python's int() reads it. The digits of a script are
/// ten contiguous code points from 0 to 9, and some scripts' sets follow each other.
fn digit(c: char) -> Option<u32> {
    if let Some(d) = c.to_digit(10) {
        return Some(d);
    }
    let nd = |c: char| get_general_category(c) == GeneralCategory::DecimalNumber;
    if !nd(c) {
        return None;
    }
    let mut start = c as u32;
    while start > 0 && char::from_u32(start - 1).is_some_and(nd) {
        start -= 1;
    }
    Some((c as u32 - start) % 10)
}

enum Units {
    NotNumber,
    TooPrecise,
    Exact(String),
}

fn canonical(digits: &str) -> String {
    let t = digits.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

/// `exact_units`: plain digits with at most one "." as a count of base units.
fn exact_units(text: &str, decimals: u32) -> Units {
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (text, None),
    };
    let digits = |s: &str| s.chars().map(digit).collect::<Option<Vec<u32>>>();
    let (Some(w), Some(f)) = (digits(whole), digits(frac.unwrap_or(""))) else {
        return Units::NotNumber;
    };
    // (\d+)(?:\.(\d*))? | \.(\d+)
    if (w.is_empty() && (frac.is_none() || f.is_empty())) || frac.is_some_and(|f| f.contains('.')) {
        return Units::NotNumber;
    }
    if f.len() > decimals as usize {
        return Units::TooPrecise;
    }
    let mut s: String = w
        .iter()
        .chain(&f)
        .map(|d| char::from_digit(*d, 10).expect("a digit"))
        .collect();
    s.extend(std::iter::repeat_n('0', decimals as usize - f.len()));
    Units::Exact(canonical(&s))
}

fn asset_of(p: &Proposal, ctx: &Context) -> Option<Chain> {
    if p.asset.is_some() {
        return p.asset.clone();
    }
    let account = ctx.accounts.iter().find(|a| a.id == p.from)?;
    matches!(account.chain, Chain::Icp | Chain::Bitcoin(_)).then(|| account.chain.clone())
}

fn asset_key(c: &Chain) -> String {
    match c {
        Chain::Icp => "icp".into(),
        Chain::Bitcoin(_) => "btc".into(),
        Chain::Evm(_) => "eth".into(),
        Chain::Icrc(ledger) => format!("icrc:{ledger}"),
    }
}

fn chain_key(c: &Chain) -> String {
    match c {
        Chain::Icp | Chain::Icrc(_) => "icp".into(),
        Chain::Bitcoin(net) => format!("btc:{net}"),
        Chain::Evm(id) => format!("evm:{}", id.map_or("None".into(), |id| id.to_string())),
    }
}

fn decimals_of(c: &Chain, ctx: &Context) -> Option<u32> {
    match c {
        Chain::Icrc(ledger) => ctx
            .tokens
            .iter()
            .find(|t| &t.ledger == ledger)
            .map(|t| t.decimals),
        Chain::Icp | Chain::Bitcoin(_) => Some(8),
        Chain::Evm(_) => Some(18),
    }
}

fn number(text: &str, units: Option<&str>, chain: Option<&Chain>, ctx: &Context) -> Verdict {
    let (Some(units), Some(chain)) = (units, chain) else {
        return Verdict::NotChecked;
    };
    let Some(decimals) = decimals_of(chain, ctx) else {
        return Verdict::NotChecked;
    };
    match exact_units(text, decimals) {
        Units::NotNumber => Verdict::NotChecked,
        Units::TooPrecise => Verdict::Differs,
        Units::Exact(v) if v == canonical(units) => Verdict::Same,
        Units::Exact(_) => Verdict::Differs,
    }
}

fn named(word: &str, aliases: &[(&str, &[&str])]) -> Vec<String> {
    aliases
        .iter()
        .filter(|(_, names)| names.contains(&word))
        .map(|(k, _)| (*k).to_owned())
        .collect()
}

fn asset_verdict(text: Option<&str>, p: &Proposal, ctx: &Context) -> Verdict {
    let (Some(text), Some(chain)) = (text, asset_of(p, ctx)) else {
        return Verdict::NotChecked;
    };
    let word = norm(text);
    let mut keys = named(&word, &ASSET_ALIASES);
    keys.extend(
        ctx.tokens
            .iter()
            .filter(|t| fold(&t.symbol) == word)
            .map(|t| format!("icrc:{}", t.ledger)),
    );
    if keys.is_empty() {
        return Verdict::NotChecked;
    }
    if keys.contains(&asset_key(&chain)) {
        Verdict::Same
    } else {
        Verdict::Differs
    }
}

fn chain_verdict(text: Option<&str>, p: &Proposal, ctx: &Context) -> Verdict {
    let (Some(text), Some(chain)) = (text, asset_of(p, ctx)) else {
        return Verdict::NotChecked;
    };
    let keys = named(&norm(text), &CHAIN_ALIASES);
    if keys.is_empty() {
        return Verdict::NotChecked;
    }
    if keys.contains(&chain_key(&chain)) {
        Verdict::Same
    } else {
        Verdict::Differs
    }
}

fn same_address(a: &str, b: &str) -> bool {
    let (la, lb) = (a.to_lowercase(), b.to_lowercase());
    if la.starts_with("0x") && lb.starts_with("0x") {
        la == lb
    } else {
        a == b
    }
}

fn is_address(text: &str) -> bool {
    identifier_kind(&text.chars().collect::<Vec<_>>()).is_some()
}

fn unique<'a>(mut found: impl Iterator<Item = &'a Account>) -> Option<&'a Account> {
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn account_by_label<'a>(text: &str, ctx: &'a Context) -> Option<&'a Account> {
    let word = norm(text);
    unique(ctx.accounts.iter().filter(|a| norm(&a.label) == word))
}

fn to_verdict(text: Option<&str>, p: &Proposal, ctx: &Context) -> Verdict {
    let Some(text) = text else {
        return Verdict::NotChecked;
    };
    let address = if is_address(text) {
        text
    } else {
        match account_by_label(text, ctx) {
            Some(a) => &a.address,
            None => return Verdict::NotChecked, // a person's name, or words the wallet cannot resolve
        }
    };
    if same_address(address, &p.to) {
        Verdict::Same
    } else {
        Verdict::Differs
    }
}

fn from_verdict(text: Option<&str>, p: &Proposal, ctx: &Context) -> Verdict {
    let Some(text) = text else {
        return Verdict::NotChecked;
    };
    let account = if is_address(text) {
        unique(
            ctx.accounts
                .iter()
                .filter(|a| same_address(&a.address, text)),
        )
    } else {
        account_by_label(text, ctx)
    };
    match account {
        None => Verdict::NotChecked,
        Some(a) if a.id == p.from => Verdict::Same,
        Some(_) => Verdict::Differs,
    }
}

fn memo_verdict(text: Option<&str>, p: &Proposal) -> Verdict {
    match text {
        None if p.memo.is_empty() => Verdict::NotChecked,
        None => Verdict::Differs,
        Some(t) if t.as_bytes() == p.memo.as_slice() => Verdict::Same,
        Some(_) => Verdict::Differs,
    }
}

fn overall(fields: Vec<(&'static str, Verdict)>, flags: Vec<&'static str>) -> Check {
    let unchecked = fields
        .iter()
        .filter(|(_, v)| *v == Verdict::NotChecked)
        .map(|(f, _)| *f)
        .collect();
    let verdict = if fields.iter().any(|(_, v)| *v == Verdict::Differs) || !flags.is_empty() {
        Overall::Differs
    } else if CORE
        .iter()
        .all(|c| fields.iter().any(|(f, v)| f == c && *v == Verdict::Same))
    {
        Overall::Matches
    } else {
        Overall::Partial
    };
    Check {
        fields,
        flags,
        overall: verdict,
        unchecked,
    }
}

/// Verdicts for one proposal. `spans` are the guard's, `[start, end)` in code points of `request`; `checked` is
/// false when the guard abstained or read the request as anything but one transfer. `flags` are the request
/// rule's.
pub fn compare(
    checked: bool,
    spans: &[(String, (usize, usize))],
    request: &str,
    ctx: &Context,
    p: &Proposal,
    flags: Vec<&'static str>,
) -> Check {
    if !checked {
        return overall(
            FIELDS.iter().map(|f| (*f, Verdict::NotChecked)).collect(),
            flags,
        );
    }
    let chars: Vec<char> = request.chars().collect();
    let texts: Vec<Option<String>> = FIELDS
        .iter()
        .map(|f| {
            spans
                .iter()
                .find(|(k, _)| k == f)
                .map(|(_, (a, b))| chars[*a..*b].iter().collect())
        })
        .collect();
    let t = |i: usize| texts[i].as_deref();
    let asset = asset_of(p, ctx);
    let fields = vec![
        (
            "amount",
            t(0).map_or(Verdict::NotChecked, |s| {
                number(s, Some(&p.amount), asset.as_ref(), ctx)
            }),
        ),
        ("asset", asset_verdict(t(1), p, ctx)),
        ("chain", chain_verdict(t(2), p, ctx)),
        ("from", from_verdict(t(3), p, ctx)),
        ("to", to_verdict(t(4), p, ctx)),
        ("memo", memo_verdict(t(5), p)),
        (
            "fee",
            t(6).map_or(Verdict::NotChecked, |s| {
                number(s, p.fee.as_deref(), asset.as_ref(), ctx)
            }),
        ),
    ];
    overall(fields, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_units_reject_excess_precision_and_read_any_decimal_digit() {
        let e = |t: &str, d| match exact_units(t, d) {
            Units::Exact(v) => v,
            Units::TooPrecise => "too precise".into(),
            Units::NotNumber => "not a number".into(),
        };
        assert_eq!(e("1.5", 8), "150000000");
        assert_eq!(e(".5", 8), "50000000");
        assert_eq!(e("0.50", 8), "50000000");
        assert_eq!(e("12.", 8), "1200000000");
        assert_eq!(e("0.000000010", 8), "too precise");
        assert_eq!(e("٣", 8), "300000000");
        assert_eq!(e("1,000", 8), "not a number");
        assert_eq!(e("1k", 8), "not a number");
        assert_eq!(e(".", 8), "not a number");
        assert_eq!(e("1.2.3", 8), "not a number");
    }
}
