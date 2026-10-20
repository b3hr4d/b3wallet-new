//! The real wallet Wasm in PocketIC. Build it first:
//! `cargo build -p wallet --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.

use candid::{Principal, decode_one, encode_one};
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::PocketIc;
use serde_bytes::ByteBuf;
use std::time::Duration;
use wallet::{ClaimArgs, InitArgs, Member, Role, Status, UpgradeState, WalletArg, WalletError};

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
