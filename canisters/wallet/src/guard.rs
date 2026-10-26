//! The guard, inside the wallet ([decision 0011](../../../docs/decisions/0011-guard-in-wallet.md)): it reads the
//! person's own words, marks where the amount, asset, recipient and the rest are (a small model, `crates/guard`),
//! and checks a pending transfer against them field by field with deterministic code, then runs stage 2's
//! keyword list over the proposer's label and the memo. Everything runs in the owners' own canister, under the code
//! its release signers signed, so an agent can neither change the model nor answer for it.
//!
//! The guard only adds flags for a person to read: nothing here approves, rejects or changes an action, and "not
//! checked" is never shown as OK.
//!
//! The model is one file, `guard.bin` (44 MB, candid-profile `guard/canister/export.py` from the round-9 release).
//! Owners write it into the guard's stable memory in chunks; `load` reads it into the heap only when its SHA-256 is
//! `FILE_SHA256`, which this code pins, so changing the model takes a release. `post_upgrade` loads it again.
//! `check` is a query and refuses replicated execution, so it is free and never charges the wallet.

use crate::{
    AccountKind, Action, ActionId, Chain, GuardField, GuardMatch, GuardOverall, GuardReport,
    GuardSpan, GuardStatus, GuardText, GuardUploadArgs, GuardVerdict, WalletError, storage,
};
use ic_stable_structures::Memory;
use serde_bytes::ByteBuf;
use sha2::{Digest, Sha256};
use std::cell::RefCell;

/// guard.bin of the round-9 release: its length and SHA-256.
pub const FILE_BYTES: u64 = 44_359_636;
pub const FILE_SHA256: [u8; 32] = [
    0xe9, 0xeb, 0xd2, 0xb0, 0x61, 0x1e, 0x61, 0x72, 0xde, 0x3c, 0x64, 0xb7, 0x05, 0x32, 0x65, 0x29,
    0xc5, 0xef, 0xcb, 0xf0, 0x19, 0xfa, 0x2d, 0x1e, 0x73, 0xaa, 0xbb, 0x0c, 0x43, 0x11, 0x58, 0xb7,
];
/// The largest chunk `upload` takes, under the IC's 2 MiB ingress limit.
pub const MAX_CHUNK: usize = 1_900_000;
const PAGE: u64 = 64 * 1024;
const READ: u64 = 1 << 20;

thread_local! {
    static GUARD: RefCell<Option<::guard::Guard>> = const { RefCell::new(None) };
}

/// Writes `chunk` at `offset` of the guard file. Writing again over the same bytes is harmless: `load` checks the
/// whole file.
pub fn upload(args: &GuardUploadArgs) -> Result<u64, WalletError> {
    let end = args
        .offset
        .checked_add(args.chunk.len() as u64)
        .filter(|end| *end <= FILE_BYTES);
    let Some(end) = end.filter(|_| args.chunk.len() <= MAX_CHUNK) else {
        return Err(WalletError::InvalidArgument(format!(
            "a chunk is at most {MAX_CHUNK} bytes and ends within the {FILE_BYTES}-byte guard file"
        )));
    };
    let memory = storage::guard_file();
    let pages = end.div_ceil(PAGE);
    if memory.size() < pages && memory.grow(pages - memory.size()) < 0 {
        return Err(WalletError::Forbidden("stable memory is full".to_string()));
    }
    memory.write(args.offset, &args.chunk);
    Ok(end)
}

/// Reads the guard file into the heap, only if it is the one this release pins.
pub fn load() -> Result<GuardStatus, WalletError> {
    let memory = storage::guard_file();
    if memory.size() * PAGE < FILE_BYTES {
        return Err(WalletError::NotFound(
            "the guard file is not uploaded".to_string(),
        ));
    }
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ as usize];
    for at in (0..FILE_BYTES).step_by(READ as usize) {
        let n = READ.min(FILE_BYTES - at) as usize;
        memory.read(at, &mut buf[..n]);
        hasher.update(&buf[..n]);
    }
    if <[u8; 32]>::from(hasher.finalize()) != FILE_SHA256 {
        return Err(WalletError::InvalidArgument(
            "the uploaded guard file is not the one this release pins".to_string(),
        ));
    }
    let invalid = |e: String| WalletError::InvalidArgument(format!("the guard file: {e}"));
    let mut head = [0u8; 12];
    memory.read(0, &mut head);
    let (len, start) = ::guard::split_file(&head).map_err(invalid)?;
    let mut header = vec![0u8; len];
    memory.read(12, &mut header);
    let header = ::guard::parse_header(&header).map_err(invalid)?;
    let values = ::guard::data_values(&header).map_err(invalid)?;
    // Free the model already loaded before reading the new one: the heap never shrinks.
    GUARD.with_borrow_mut(|g| *g = None);
    let mut data = Vec::with_capacity(values);
    for at in (0..4 * values as u64).step_by(READ as usize) {
        let n = READ.min(4 * values as u64 - at) as usize;
        memory.read(start as u64 + at, &mut buf[..n]);
        data.extend(
            buf[..n]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        );
    }
    let loaded = ::guard::Guard::from_parts(&header, data).map_err(invalid)?;
    GUARD.with_borrow_mut(|g| *g = Some(loaded));
    Ok(status())
}

pub fn status() -> GuardStatus {
    GUARD.with_borrow(|g| GuardStatus {
        file_sha256: ByteBuf::from(FILE_SHA256.to_vec()),
        file_bytes: FILE_BYTES,
        loaded: g.is_some(),
        release: g.as_ref().map(|g| g.release.clone()),
        model_sha256: g.as_ref().map(|g| g.model_sha256.clone()),
    })
}

fn chain(c: &Chain) -> ::guard::Chain {
    match c {
        Chain::Icp => ::guard::Chain::Icp,
        Chain::Icrc(ledger) => ::guard::Chain::Icrc(ledger.to_text()),
        Chain::Bitcoin(network) => ::guard::Chain::Bitcoin(format!("{network:?}")),
        Chain::Evm(id) => ::guard::Chain::Evm(Some(*id)),
    }
}

/// The wallet as the comparator reads it: the listed accounts, with their labels and addresses.
// shortcut: no tokens, since the wallet keeps no ICRC-1 ledger's symbol or decimals; an ICRC-1 transfer's asset
// and amount are then not checked. Upgrade by storing them when a ledger is tracked.
fn context() -> ::guard::Context {
    let wallet = ic_cdk::api::canister_self();
    let accounts = storage::accounts()
        .into_iter()
        .filter(|(_, account)| account.is_listed())
        .map(|(id, account)| {
            let view = account.view(id, wallet);
            let chain = match view.kind {
                AccountKind::InternetComputer => ::guard::Chain::Icp,
                AccountKind::Bitcoin(network) => ::guard::Chain::Bitcoin(format!("{network:?}")),
                AccountKind::Evm => ::guard::Chain::Evm(None),
            };
            ::guard::Account {
                id,
                label: view.label,
                address: view.address,
                chain,
            }
        })
        .collect();
    ::guard::Context {
        accounts,
        tokens: Vec::new(),
    }
}

fn text(name: &str, text: &str, flags: ::guard::Flags) -> GuardText {
    GuardText {
        name: name.to_string(),
        text: text.to_string(),
        flags: flags.flags.iter().map(|f| (*f).to_string()).collect(),
        matched: flags
            .matched
            .into_iter()
            .map(|m| GuardMatch {
                cue: m.cue.to_string(),
                start: m.start as u32,
                end: m.end as u32,
                text: m.text,
            })
            .collect(),
    }
}

/// The guard's report on transfer `id` against the words of the owner's request it answers.
pub fn check(id: ActionId) -> Result<GuardReport, WalletError> {
    if ic_cdk::api::in_replicated_execution() {
        return Err(WalletError::Forbidden(
            "guard_check answers only as a query".to_string(),
        ));
    }
    let pending =
        storage::action(id).ok_or_else(|| WalletError::NotFound(format!("action {id}")))?;
    let Action::Transfer(t) = &pending.action else {
        return Err(WalletError::InvalidArgument(format!(
            "action {id} is not a transfer"
        )));
    };
    // The words come from the owner's own request, never from the agent: a transfer that answers none is not checked.
    let request = pending
        .request
        .and_then(storage::request)
        .ok_or_else(|| WalletError::NotFound(format!("action {id} answers no request of yours")))?
        .text;
    GUARD.with_borrow(|g| {
        let g = g
            .as_ref()
            .ok_or_else(|| WalletError::NotFound("the guard model is not loaded".to_string()))?;
        // The wallet's own account labels are swapped before the model reads (candid-profile D-045), so a label such
        // as "BTC Savings" reads as a label, not as a second asset; the comparator resolves it from the owner's words.
        let ctx = context();
        let names: Vec<String> = ctx.accounts.iter().map(|a| a.label.clone()).collect();
        let symbols: Vec<String> = ctx.tokens.iter().map(|t| t.symbol.clone()).collect();
        let answer = g
            .answer_named(&request, &names, &symbols)
            .map_err(WalletError::InvalidArgument)?;
        let proposal = ::guard::Proposal {
            from: t.from,
            asset: t.asset.as_ref().map(chain),
            to: t.to.clone(),
            amount: t.amount.0.to_string(),
            fee: t.fee.as_ref().map(|f| f.0.to_string()),
            memo: t.memo.as_ref().map(|m| m.to_vec()).unwrap_or_default(),
        };
        let checked = g.compare(&answer, &request, &ctx, &proposal);
        let mut texts = Vec::new();
        if let Some(member) = storage::member(pending.proposed_by) {
            texts.push(text("label", &member.label, g.manip(&member.label)));
        }
        if let Some(memo) = t.memo.as_ref().and_then(|m| std::str::from_utf8(m).ok()) {
            texts.push(text("memo", memo, g.manip(memo)));
        }
        let chars: Vec<char> = request.chars().collect();
        Ok(GuardReport {
            request: request.clone(),
            class: answer.class.clone(),
            abstain: answer.abstain,
            confidence: answer.confidence,
            spans: answer
                .spans
                .iter()
                .map(|(slot, (a, b))| GuardSpan {
                    slot: slot.clone(),
                    start: *a as u32,
                    end: *b as u32,
                    text: chars[*a..*b].iter().collect(),
                })
                .collect(),
            fields: checked
                .fields
                .iter()
                .map(|(field, v)| GuardField {
                    field: (*field).to_string(),
                    verdict: match v {
                        ::guard::Verdict::Same => GuardVerdict::Same,
                        ::guard::Verdict::Differs => GuardVerdict::Differs,
                        ::guard::Verdict::NotChecked => GuardVerdict::NotChecked,
                    },
                })
                .collect(),
            request_flags: checked.flags.iter().map(|f| (*f).to_string()).collect(),
            overall: match checked.overall {
                ::guard::Overall::Differs => GuardOverall::Differs,
                ::guard::Overall::Matches => GuardOverall::Matches,
                ::guard::Overall::Partial => GuardOverall::Partial,
            },
            texts,
            instructions: ic_cdk::api::performance_counter(0),
        })
    })
}
