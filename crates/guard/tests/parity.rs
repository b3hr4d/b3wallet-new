//! The port against the shipped Python guard (candid-profile `packages/profile-router/guard/`).
//!
//! `checks`: the comparator, the request rule and the cut keyword list against the answers the Python code gave
//! (`tests/data/checks.jsonl`, from candid-profile `guard/canister/fixtures.py --every 60` on round 9's test split
//! and stage 2's test texts). GUARD_CHECKS names another such file, such as the full one (`fixtures.py` without
//! `--every`: 12,000 requests, 56,743 proposals, 5,150 texts).
//!
//! `model`: with GUARD_BIN (a guard.bin) and GUARD_PARITY (the release's parity.json, from PyTorch), the inputs,
//! the logits (within 1e-3) and the answers of all its cases; skipped without them, since the weights (44 MB) are
//! not in the repository.

use guard::{
    Account, Chain, Context, Guard, Manip, Overall, Piece, Proposal, RequestRule, Token, Verdict,
    compare,
};
use serde_json::{Value, json};
use std::path::PathBuf;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn chain(v: &Value) -> Chain {
    let (k, x) = v
        .as_object()
        .and_then(|o| o.iter().next())
        .expect("a chain");
    match k.as_str() {
        "Icp" => Chain::Icp,
        "Icrc" => Chain::Icrc(x.as_str().expect("a ledger").into()),
        "Bitcoin" => Chain::Bitcoin(x.as_str().expect("a network").into()),
        "Evm" => Chain::Evm(x.as_u64()),
        other => panic!("chain {other}"),
    }
}

fn context(v: &Value) -> Context {
    Context {
        accounts: v["accounts"]
            .as_array()
            .expect("accounts")
            .iter()
            .map(|a| Account {
                id: a["id"].as_u64().expect("an id") as u32,
                label: a["label"].as_str().expect("a label").into(),
                address: a["address"].as_str().expect("an address").into(),
                chain: chain(&a["chain"]),
            })
            .collect(),
        tokens: v["tokens"]
            .as_array()
            .expect("tokens")
            .iter()
            .map(|t| Token {
                ledger: t["ledger"].as_str().expect("a ledger").into(),
                symbol: t["symbol"].as_str().expect("a symbol").into(),
                decimals: t["decimals"].as_u64().expect("decimals") as u32,
            })
            .collect(),
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn proposal(v: &Value) -> Proposal {
    Proposal {
        from: v["from"].as_u64().expect("from") as u32,
        asset: (!v["asset"].is_null()).then(|| chain(&v["asset"])),
        to: v["to"].as_str().expect("to").into(),
        amount: v["amount"].as_str().expect("amount").into(),
        fee: v["fee"].as_str().map(str::to_owned),
        memo: v["memo"].as_str().map(hex).unwrap_or_default(),
    }
}

fn verdict(v: Verdict) -> &'static str {
    match v {
        Verdict::Same => "same",
        Verdict::Differs => "differs",
        Verdict::NotChecked => "not_checked",
    }
}

fn overall(o: Overall) -> &'static str {
    match o {
        Overall::Differs => "differs",
        Overall::Matches => "matches",
        Overall::Partial => "partial",
    }
}

#[test]
fn checks() {
    let vocab: Vec<String> = std::fs::read_to_string(data("vocab.txt"))
        .expect("vocab.txt")
        .split('\n')
        .map(str::to_owned)
        .collect();
    let rule = RequestRule::new(&vocab);
    let manip = Manip::default();
    let path = std::env::var("GUARD_CHECKS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data("checks.jsonl"));
    let text = std::fs::read_to_string(&path).expect("the checks file");
    let (mut n, mut wrong) = ([0usize; 3], Vec::new());
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let c: Value = serde_json::from_str(line).expect("a JSON line");
        match c["kind"].as_str() {
            Some("compare") => {
                let request = c["request"].as_str().expect("a request");
                let g = &c["guard"];
                let spans: Vec<(String, (usize, usize))> = g["spans"]
                    .as_object()
                    .expect("spans")
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            (
                                v[0].as_u64().unwrap() as usize,
                                v[1].as_u64().unwrap() as usize,
                            ),
                        )
                    })
                    .collect();
                let checked = g["abstain"] == false && g["class"] == "one_transfer";
                let ctx = context(&c["context"]);
                for pair in c["pairs"].as_array().expect("pairs") {
                    n[0] += 1;
                    let got = compare(
                        checked,
                        &spans,
                        request,
                        &ctx,
                        &proposal(&pair["proposal"]),
                        rule.flags(request),
                    );
                    let fields: serde_json::Map<String, Value> = got
                        .fields
                        .iter()
                        .map(|(f, v)| ((*f).to_owned(), json!(verdict(*v))))
                        .collect();
                    let got = json!({ "fields": fields, "flags": got.flags, "overall": overall(got.overall), "unchecked": got.unchecked });
                    if got != pair["want"] {
                        wrong.push(format!(
                            "compare {request:?} {}\n  rust {got}\n  want {}",
                            pair["proposal"], pair["want"]
                        ));
                    }
                }
            }
            Some("request") => {
                n[1] += 1;
                let request = c["request"].as_str().expect("a request");
                if json!(rule.flags(request)) != c["want"] {
                    wrong.push(format!(
                        "request {request:?}: rust {:?}, want {}",
                        rule.flags(request),
                        c["want"]
                    ));
                }
            }
            Some("manip") => {
                n[2] += 1;
                let t = c["text"].as_str().expect("a text");
                let f = manip.flag(t);
                let matched: Vec<Value> = f
                    .matched
                    .iter()
                    .map(
                        |m| json!({ "cue": m.cue, "start": m.start, "end": m.end, "text": m.text }),
                    )
                    .collect();
                let got = json!({ "flags": f.flags, "matched": matched });
                if got != c["want"] {
                    wrong.push(format!("manip {t:?}\n  rust {got}\n  want {}", c["want"]));
                }
            }
            other => panic!("kind {other:?}"),
        }
    }
    eprintln!(
        "{} proposals, {} requests, {} texts from {}",
        n[0],
        n[1],
        n[2],
        path.display()
    );
    assert!(n.iter().all(|&k| k > 0), "every kind is checked");
    assert!(
        wrong.is_empty(),
        "{} of {} differ:\n{}",
        wrong.len(),
        n.iter().sum::<usize>(),
        wrong
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn model() {
    let (Ok(bin), Ok(parity)) = (std::env::var("GUARD_BIN"), std::env::var("GUARD_PARITY")) else {
        eprintln!("skipped: set GUARD_BIN and GUARD_PARITY to check the model");
        return;
    };
    let guard = Guard::from_bytes(&std::fs::read(&bin).expect("guard.bin")).expect("a guard file");
    let p: Value =
        serde_json::from_slice(&std::fs::read(&parity).expect("parity.json")).expect("JSON");
    assert_eq!(
        p["model_safetensors_sha256"].as_str(),
        Some(guard.model_sha256.as_str())
    );
    let floats = |v: &Value| {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_f64)
                    .map(|x| x as f32)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let (mut max_diff, mut wrong) = (0f32, Vec::new());
    let cases = p["cases"].as_array().expect("cases");
    for c in cases {
        let request = c["request"].as_str().expect("a request");
        let text: Vec<char> = request.chars().collect();
        let (ids, pieces) = guard.tokenizer.encode(&text).expect("fits");
        let want_pieces: Vec<Piece> = c["pieces"]
            .as_array()
            .expect("pieces")
            .iter()
            .map(|q| Piece {
                id: q[0].as_u64().unwrap() as u32,
                start: q[1].as_u64().unwrap() as usize,
                end: q[2].as_u64().unwrap() as usize,
            })
            .collect();
        let want_ids: Vec<u32> = c["ids"]
            .as_array()
            .expect("ids")
            .iter()
            .filter_map(Value::as_u64)
            .map(|x| x as u32)
            .collect();
        assert_eq!((&ids, &pieces), (&want_ids, &want_pieces), "{request:?}");
        let want_tags: Vec<f32> = c["tag_logits"]
            .as_array()
            .expect("tag logits")
            .iter()
            .flat_map(floats)
            .collect();
        let (class_logits, tag_logits) = guard.model.forward(&ids, &vec![0; ids.len()]);
        for (a, b) in class_logits
            .iter()
            .chain(&tag_logits)
            .zip(floats(&c["class_logits"]).iter().chain(&want_tags))
        {
            max_diff = max_diff.max((a - b).abs());
        }
        let got = guard.answer(request).expect("an answer");
        let want = &c["output"];
        let spans: serde_json::Map<String, Value> = got
            .spans
            .iter()
            .map(|(k, (a, b))| (k.clone(), json!([a, b])))
            .collect();
        if want["class"] != got.class.as_str()
            || want["spans"] != Value::Object(spans)
            || want["abstain"] != got.abstain
            || (want["confidence"].as_f64().unwrap() - f64::from(got.confidence)).abs() > 1e-4
        {
            wrong.push(format!("{request:?}: rust {got:?}, want {want}"));
        }
    }
    eprintln!("{} cases, largest logit difference {max_diff}", cases.len());
    assert!(max_diff <= 1e-3, "logits differ by {max_diff}");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
