//! The real wallet Wasm in PocketIC. Build it first:
//! `cargo build -p wallet --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.

use candid::types::{FuncMode, TypeInner};
use candid::{Principal, decode_one, encode_args, encode_one};
use candid_parser::utils::CandidSource;
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::{ErrorCode, PocketIc, RejectCode, RejectResponse};
use serde_bytes::ByteBuf;
use std::path::Path;
use std::time::Duration;
use wallet::{
    ClaimArgs, InitArgs, Member, NOT_A_MEMBER, NOT_CLAIMED, Role, Status, UpgradeState, WalletArg,
    WalletError,
};

const DAY_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

fn wasm() -> Vec<u8> {
    let path = std::env::var("WALLET_WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/wasm32-unknown-unknown/release/wallet.wasm"
        )
        .to_string()
    });
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("cannot read {path} ({e}); build it with `cargo build -p wallet --target wasm32-unknown-unknown --release`")
    })
}

fn release_channel() -> Principal {
    Principal::from_text("rrkah-fqaaa-aaaaa-aaaaq-cai").unwrap()
}

fn public_key(key: &SigningKey) -> ByteBuf {
    ByteBuf::from(key.verifying_key().to_bytes().to_vec())
}

/// The keypair a claim link would carry.
fn claim_key() -> SigningKey {
    SigningKey::from_bytes(&[1; 32])
}

fn user(seed: u8) -> Principal {
    Principal::self_authenticating([seed; 32])
}

fn init_args(pic: &PocketIc) -> InitArgs {
    InitArgs {
        claim_public_key: public_key(&claim_key()),
        claim_expires_at_ns: pic.get_time().as_nanos_since_unix_epoch() + DAY_NS,
        release_channel: release_channel(),
        release_signers: vec![
            public_key(&SigningKey::from_bytes(&[2; 32])),
            public_key(&SigningKey::from_bytes(&[3; 32])),
        ],
        release_threshold: 2,
        factory: None,
    }
}

fn empty_canister(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    id
}

/// A fresh install that reports failure instead of panicking.
fn try_install(pic: &PocketIc, id: Principal, arg: &WalletArg) -> Result<(), String> {
    pic.reinstall_canister(id, wasm(), encode_one(arg).unwrap(), None)
        .map_err(|reject| format!("{reject:?}"))
}

fn status(pic: &PocketIc, id: Principal) -> Status {
    let reply = pic
        .query_call(
            id,
            Principal::anonymous(),
            "status",
            encode_one(()).unwrap(),
        )
        .expect("status answers");
    decode_one(&reply).unwrap()
}

fn installed_wallet(pic: &PocketIc) -> Principal {
    let id = empty_canister(pic);
    try_install(pic, id, &WalletArg::Init(init_args(pic))).unwrap();
    id
}

/// What the browser signs with the claim key for `caller` on `wallet`.
fn claim_signature(wallet: Principal, caller: Principal) -> ByteBuf {
    let message = wallet::claim_message(wallet, caller);
    ByteBuf::from(claim_key().sign(message.as_bytes()).to_bytes().to_vec())
}

fn claim(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    signature: ByteBuf,
) -> Result<Member, WalletError> {
    let reply = pic
        .update_call(
            id,
            caller,
            "claim",
            encode_one(ClaimArgs { signature }).unwrap(),
        )
        .expect("claim answers");
    decode_one(&reply).unwrap()
}

#[test]
fn status_reports_the_install_arguments() {
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    let args = init_args(&pic);
    try_install(&pic, id, &WalletArg::Init(args.clone())).unwrap();

    let status = status(&pic, id);
    assert_eq!(status.version, "0.1.0");
    assert!(!status.claimed);
    assert_eq!(status.claim_expires_at_ns, Some(args.claim_expires_at_ns));
    assert_eq!(status.release_channel, release_channel());
    assert_eq!(status.upgrade, UpgradeState::Idle);
    assert!(status.cycles > 0u64, "cycles: {}", status.cycles);
}

#[test]
fn install_without_init_arguments_is_refused() {
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    let err = try_install(&pic, id, &WalletArg::Upgrade).unwrap_err();
    assert!(err.contains("install needs WalletArg::Init"), "{err}");
}

#[test]
fn install_with_invalid_arguments_is_refused() {
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    let args = InitArgs {
        claim_public_key: ByteBuf::from(vec![1; 31]),
        ..init_args(&pic)
    };
    let err = try_install(&pic, id, &WalletArg::Init(args)).unwrap_err();
    assert!(err.contains("invalid InitArgs: claim_public_key"), "{err}");
}

/// An upgrade in a later round. The IC rate-limits `install_code` on a
/// canister whose previous installs used many instructions, and each round
/// pays some of that back.
fn upgrade(pic: &PocketIc, id: Principal, arg: &WalletArg) -> Result<(), String> {
    pic.tick();
    pic.upgrade_canister(id, wasm(), encode_one(arg).unwrap(), None)
        .map_err(|reject| format!("{reject:?}"))
}

#[test]
fn upgrades_keep_the_state_in_stable_memory() {
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    try_install(&pic, id, &WalletArg::Init(init_args(&pic))).unwrap();
    let before = status(&pic, id);

    for _ in 0..2 {
        upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
        let after = status(&pic, id);
        assert_eq!(after.claim_expires_at_ns, before.claim_expires_at_ns);
        assert_eq!(after.release_channel, before.release_channel);
        assert_eq!(after.claimed, before.claimed);
        assert_eq!(after.upgrade, UpgradeState::Idle);
    }
}

#[test]
fn an_upgrade_with_init_arguments_is_refused() {
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    let args = init_args(&pic);
    try_install(&pic, id, &WalletArg::Init(args.clone())).unwrap();

    let other = InitArgs {
        release_channel: Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap(),
        ..args.clone()
    };
    let err = upgrade(&pic, id, &WalletArg::Init(other)).unwrap_err();
    assert!(err.contains("an upgrade needs WalletArg::Upgrade"), "{err}");
    assert_eq!(status(&pic, id).release_channel, args.release_channel);
}

/// A canister whose stable memory holds no wallet state, as the step 1 Wasm
/// left it, refuses the upgrade and keeps its running version.
#[test]
fn an_upgrade_without_stored_state_is_refused() {
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    let empty_module = b"\0asm\x01\0\0\0".to_vec();
    pic.install_canister(id, empty_module, vec![], None);
    let module_hash = pic.canister_status(id, None).unwrap().module_hash;

    let err = upgrade(&pic, id, &WalletArg::Upgrade).unwrap_err();
    assert!(
        err.contains("the upgrade cannot read the state: stable memory holds no configuration"),
        "{err}"
    );
    assert_eq!(
        pic.canister_status(id, None).unwrap().module_hash,
        module_hash
    );
}

#[test]
fn the_claim_makes_the_caller_the_first_owner() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let owner = user(7);

    let member = claim(&pic, id, owner, claim_signature(id, owner)).unwrap();
    assert_eq!(member.id, owner);
    assert_eq!(member.role, Role::Owner);
    assert_eq!(member.added_by, None);
    assert_eq!(
        member.added_at_ns,
        pic.get_time().as_nanos_since_unix_epoch()
    );

    let status = status(&pic, id);
    assert!(status.claimed);
    assert_eq!(status.claim_expires_at_ns, None);
}

#[test]
fn a_signature_for_another_principal_is_refused() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let (owner, thief) = (user(7), user(8));
    let signature = claim_signature(id, owner);

    // Someone who saw the signature in flight cannot use it.
    assert_eq!(
        claim(&pic, id, thief, signature.clone()),
        Err(WalletError::BadSignature)
    );
    assert!(!status(&pic, id).claimed);
    // The failed attempt used nothing up.
    assert!(claim(&pic, id, owner, signature).is_ok());
}

#[test]
fn a_replayed_signature_and_a_second_claim_are_refused() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let (owner, second) = (user(7), user(8));
    let signature = claim_signature(id, owner);
    claim(&pic, id, owner, signature.clone()).unwrap();

    assert_eq!(
        claim(&pic, id, owner, signature),
        Err(WalletError::AlreadyClaimed)
    );
    assert_eq!(
        claim(&pic, id, second, claim_signature(id, second)),
        Err(WalletError::AlreadyClaimed)
    );
}

#[test]
fn an_expired_claim_is_refused() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let owner = user(7);
    pic.advance_time(Duration::from_nanos(DAY_NS));

    assert_eq!(
        claim(&pic, id, owner, claim_signature(id, owner)),
        Err(WalletError::ClaimExpired)
    );
    assert!(!status(&pic, id).claimed);
}

#[test]
fn the_anonymous_principal_cannot_claim() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let anonymous = Principal::anonymous();

    assert!(matches!(
        claim(&pic, id, anonymous, claim_signature(id, anonymous)),
        Err(WalletError::Forbidden(_))
    ));
    assert!(!status(&pic, id).claimed);
}

#[test]
fn a_signature_for_another_wallet_is_refused() {
    let pic = PocketIc::new();
    // Two wallets installed with the same claim key.
    let (first, second) = (installed_wallet(&pic), installed_wallet(&pic));
    let owner = user(7);

    assert_eq!(
        claim(&pic, second, owner, claim_signature(first, owner)),
        Err(WalletError::BadSignature)
    );
    assert!(claim(&pic, first, owner, claim_signature(first, owner)).is_ok());
}

#[test]
fn the_claim_survives_an_upgrade() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let (owner, second) = (user(7), user(8));
    claim(&pic, id, owner, claim_signature(id, owner)).unwrap();

    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
    let status = status(&pic, id);
    assert!(status.claimed);
    assert_eq!(status.claim_expires_at_ns, None);
    assert_eq!(
        claim(&pic, id, second, claim_signature(id, second)),
        Err(WalletError::AlreadyClaimed)
    );
}

fn whoami(pic: &PocketIc, id: Principal, caller: Principal) -> Option<Member> {
    let reply = pic
        .query_call(id, caller, "whoami", encode_one(()).unwrap())
        .expect("whoami answers");
    decode_one(&reply).unwrap()
}

fn members(pic: &PocketIc, id: Principal, caller: Principal) -> Result<Vec<Member>, WalletError> {
    let reply = pic
        .query_call(id, caller, "members", encode_one(()).unwrap())
        .expect("members answers");
    decode_one(&reply).unwrap()
}

#[test]
fn whoami_and_members_answer_the_owner_also_after_an_upgrade() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let owner = user(7);
    let member = claim(&pic, id, owner, claim_signature(id, owner)).unwrap();

    for upgraded in [false, true] {
        if upgraded {
            upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
        }
        assert_eq!(whoami(&pic, id, owner), Some(member.clone()));
        assert_eq!(members(&pic, id, owner), Ok(vec![member.clone()]));
    }
}

#[test]
fn whoami_tells_a_non_member_that_it_is_not_one() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let owner = user(7);
    claim(&pic, id, owner, claim_signature(id, owner)).unwrap();

    assert_eq!(whoami(&pic, id, user(8)), None);
    assert_eq!(whoami(&pic, id, Principal::anonymous()), None);
}

/// What anyone may call before the claim. Every other method is refused.
const PUBLIC_BEFORE_CLAIM: &[&str] = &["claim", "status", "http_request"];
/// What anyone may call once the wallet is claimed. Every other method is for
/// members only.
const PUBLIC_AFTER_CLAIM: &[&str] = &["claim", "status", "http_request", "whoami", "request_join"];

/// Every method of `candid/wallet.did` in name order, and whether it is a
/// query.
fn target_methods() -> Vec<(String, bool)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/wallet.did");
    let (env, actor) = CandidSource::File(Path::new(path))
        .load()
        .expect("candid/wallet.did parses");
    let actor = env.trace_type(&actor.expect("a service")).unwrap();
    let TypeInner::Class(_, service) = actor.as_ref() else {
        panic!("the service has no init arguments: {actor}");
    };
    env.as_service(service)
        .unwrap()
        .iter()
        .map(|(name, method)| {
            let modes = &env.as_func(method).unwrap().modes;
            let query = modes
                .iter()
                .any(|mode| matches!(mode, FuncMode::Query | FuncMode::CompositeQuery));
            (name.clone(), query)
        })
        .collect()
}

/// A call without arguments: a method that runs at all fails to decode them,
/// so only a guard, which runs before decoding, can answer with its message.
fn call_without_arguments(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
    query: bool,
) -> Result<Vec<u8>, RejectResponse> {
    let arg = encode_args(()).unwrap();
    if query {
        pic.query_call(id, caller, method, arg)
    } else {
        pic.update_call(id, caller, method, arg)
    }
}

/// Calls, as `caller`, every method of the target interface that is not in
/// `public`, and checks that each one is refused with `refusal` or is not
/// implemented yet. Returns the methods it refused.
fn methods_refused(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    public: &[&str],
    refusal: &str,
) -> Vec<String> {
    let methods = target_methods();
    for name in public {
        assert!(
            methods.iter().any(|(method, _)| method == name),
            "{name} is not in candid/wallet.did"
        );
    }
    let (mut refused, mut not_yet, mut wrong) = (Vec::new(), Vec::new(), Vec::new());
    for (method, query) in methods {
        if public.contains(&method.as_str()) {
            continue;
        }
        match call_without_arguments(pic, id, caller, &method, query) {
            Err(reject) if reject.error_code == ErrorCode::CanisterMethodNotFound => {
                not_yet.push(method)
            }
            Err(reject)
                if reject.reject_code == RejectCode::CanisterReject
                    && reject.reject_message == refusal =>
            {
                refused.push(method)
            }
            other => wrong.push(format!("{method}: {other:?}")),
        }
    }
    assert!(
        wrong.is_empty(),
        "not refused with {refusal:?}:\n{}",
        wrong.join("\n")
    );
    eprintln!(
        "refused: {}; not implemented yet: {}",
        refused.join(", "),
        not_yet.join(", ")
    );
    refused
}

#[test]
fn before_the_claim_only_the_public_methods_answer() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);

    for caller in [user(8), Principal::anonymous()] {
        let refused = methods_refused(&pic, id, caller, PUBLIC_BEFORE_CLAIM, NOT_CLAIMED);
        assert_eq!(refused, ["members", "whoami"]);
    }
}

#[test]
fn a_non_member_is_refused_by_every_method_for_members() {
    let pic = PocketIc::new();
    let id = installed_wallet(&pic);
    let owner = user(7);
    claim(&pic, id, owner, claim_signature(id, owner)).unwrap();

    for caller in [user(8), Principal::anonymous()] {
        let refused = methods_refused(&pic, id, caller, PUBLIC_AFTER_CLAIM, NOT_A_MEMBER);
        assert_eq!(refused, ["members"]);
    }
}
