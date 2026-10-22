//! The real wallet Wasm in PocketIC. Build it first:
//! `cargo build -p wallet --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.

use candid::types::{FuncMode, TypeInner};
use candid::{CandidType, Deserialize, Principal, decode_one, encode_args, encode_one};
use candid_parser::utils::CandidSource;
use ed25519_dalek::{Signer, SigningKey};
use ic_cdk_bitcoin_canister::{
    Config, Fees, GetUtxosRequest, GetUtxosResponse, NetworkInRequest, SetConfigRequest,
};
use ic_http_certification::{HttpRequest, StatusCode};
use ic_response_verification::types::VerifiedResponse;
use ic_response_verification::verify_request_response_pair;
use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
use pocket_ic::common::rest::{IcpFeatures, IcpFeaturesConfig};
use pocket_ic::{ErrorCode, PocketIc, PocketIcBuilder, RejectCode, RejectResponse};
use serde_bytes::ByteBuf;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};
use wallet::{
    Account, AccountHolder, AccountKind, Action, ActionId, ActionKind, ActionState, AddAccountArgs,
    AuditEntry, AuditEvent, AuditRequest, Balance, BitcoinNetwork, Chain, ClaimArgs, DailyLimit,
    InitArgs, Member, NOT_A_MEMBER, NOT_A_PROPOSER, NOT_AN_APPROVER, NOT_AN_OWNER, NOT_CLAIMED,
    Outcome, Pending, Policy, Role, Status, TransferRequest, UpgradeArgs, UpgradeState, WalletArg,
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
        bitcoin_network: None,
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
    let err = try_install(&pic, id, &WalletArg::Upgrade(None)).unwrap_err();
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
        upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
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

    let err = upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap_err();
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

    upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
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
            upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
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
        assert_eq!(
            refused,
            [
                "accounts",
                "add_account",
                "approve",
                "assets",
                "audit_log",
                "balance",
                "execute",
                "members",
                "pending",
                "policy",
                "propose",
                "reject",
                "release_available",
                "request_join",
                "track_asset",
                "untrack_asset",
                "upgrade_status",
                "whoami"
            ]
        );
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
        assert_eq!(
            refused,
            [
                "accounts",
                "add_account",
                "approve",
                "assets",
                "audit_log",
                "balance",
                "execute",
                "members",
                "pending",
                "policy",
                "propose",
                "reject",
                "release_available",
                "track_asset",
                "untrack_asset",
                "upgrade_status"
            ]
        );
    }
}

// ---- Step 5: the policy and pending actions ----------------------------------

const HOUR_NS: u64 = 60 * 60 * 1_000_000_000;

fn update<R: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
    arg: &impl CandidType,
) -> R {
    let reply = pic
        .update_call(id, caller, method, encode_one(arg).unwrap())
        .unwrap_or_else(|reject| panic!("{method} answers: {reject}"));
    decode_one(&reply).unwrap()
}

fn query<R: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
) -> R {
    let reply = pic
        .query_call(id, caller, method, encode_one(()).unwrap())
        .unwrap_or_else(|reject| panic!("{method} answers: {reject}"));
    decode_one(&reply).unwrap()
}

type PendingResult = Result<Pending, WalletError>;

fn propose(pic: &PocketIc, id: Principal, caller: Principal, action: Action) -> PendingResult {
    update(pic, id, caller, "propose", &action)
}

/// `approve`, `reject` or `execute`.
fn act(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
    action: ActionId,
) -> PendingResult {
    update(pic, id, caller, method, &action)
}

fn pending(pic: &PocketIc, id: Principal, caller: Principal) -> Vec<Pending> {
    query::<Result<Vec<Pending>, WalletError>>(pic, id, caller, "pending").unwrap()
}

fn policy(pic: &PocketIc, id: Principal, caller: Principal) -> Policy {
    query::<Result<Policy, WalletError>>(pic, id, caller, "policy").unwrap()
}

/// A wallet claimed by `user(7)`.
fn claimed_wallet(pic: &PocketIc) -> (Principal, Principal) {
    let id = installed_wallet(pic);
    let owner = user(7);
    claim(pic, id, owner, claim_signature(id, owner)).unwrap();
    (id, owner)
}

fn add_member_action(id: Principal, role: Role) -> Action {
    add_member_action_labelled(id, role, "device")
}

fn add_member_action_labelled(id: Principal, role: Role, label: &str) -> Action {
    Action::AddMember {
        id,
        role,
        label: label.to_string(),
    }
}

/// Proposes and executes an action whose rule needs one approval and no
/// timelock.
fn run_at_once(pic: &PocketIc, id: Principal, owner: Principal, action: Action) {
    let proposed = propose(pic, id, owner, action).unwrap();
    assert_eq!(proposed.state, ActionState::Ready);
    let executed = act(pic, id, owner, "execute", proposed.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
}

#[test]
fn a_new_wallet_has_the_default_policy() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let policy = policy(&pic, id, owner);
    assert_eq!(policy.rules.len(), 8);
    assert!(policy.rules.iter().all(|rule| rule.quorum == 1));
    let timelock = |kind: &str| {
        policy
            .rules
            .iter()
            .find(|rule| format!("{:?}", rule.kind) == kind)
            .unwrap()
            .timelock_ns
    };
    assert_eq!(timelock("AddMember"), 0);
    assert_eq!(timelock("RemoveMember"), 24 * HOUR_NS);
    assert_eq!(timelock("SetPolicy"), 7 * DAY_NS);
    assert_eq!(timelock("Upgrade"), 7 * DAY_NS);
}

#[test]
fn an_owner_adds_a_member_at_once() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let laptop = user(8);

    let proposed = propose(&pic, id, owner, add_member_action(laptop, Role::Approver)).unwrap();
    assert_eq!(proposed.id, 1);
    assert_eq!(proposed.approvals, [owner]);
    assert_eq!(proposed.state, ActionState::Ready);
    assert_eq!(pending(&pic, id, owner), std::slice::from_ref(&proposed));

    let executed = act(&pic, id, owner, "execute", proposed.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    let member = whoami(&pic, id, laptop).unwrap();
    assert_eq!(member.role, Role::Approver);
    assert_eq!(member.added_by, Some(owner));
    assert_eq!(members(&pic, id, owner).unwrap().len(), 2);
    assert_eq!(pending(&pic, id, owner), []);
    assert!(matches!(
        act(&pic, id, owner, "execute", proposed.id),
        Err(WalletError::InvalidArgument(reason)) if reason == "action 1 is already executed"
    ));
}

#[test]
fn removing_a_member_waits_for_its_timelock_also_across_an_upgrade() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let laptop = user(8);
    run_at_once(&pic, id, owner, add_member_action(laptop, Role::Approver));

    let proposed = propose(&pic, id, owner, Action::RemoveMember(laptop)).unwrap();
    assert_eq!(proposed.state, ActionState::Ready);
    assert_eq!(
        proposed.executable_after_ns,
        proposed.proposed_at_ns + 24 * HOUR_NS
    );
    assert_eq!(
        act(&pic, id, owner, "execute", proposed.id),
        Err(WalletError::TimelockActive {
            executable_after_ns: proposed.executable_after_ns
        })
    );

    upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
    assert_eq!(pending(&pic, id, owner), std::slice::from_ref(&proposed));
    pic.advance_time(Duration::from_nanos(23 * HOUR_NS));
    assert!(matches!(
        act(&pic, id, owner, "execute", proposed.id),
        Err(WalletError::TimelockActive { .. })
    ));

    pic.advance_time(Duration::from_nanos(HOUR_NS));
    let executed = act(&pic, id, laptop, "execute", proposed.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    assert_eq!(whoami(&pic, id, laptop), None);
}

/// Raises every quorum to two, which needs a second member who can approve
/// and the SetPolicy timelock of seven days. Returns that member.
fn quorum_of_two(pic: &PocketIc, id: Principal, owner: Principal) -> Principal {
    let approver = user(8);
    run_at_once(pic, id, owner, add_member_action(approver, Role::Approver));
    let mut policy = policy(pic, id, owner);
    for rule in &mut policy.rules {
        rule.quorum = 2;
    }
    let proposed = propose(pic, id, owner, Action::SetPolicy(policy.clone())).unwrap();
    assert_eq!(proposed.state, ActionState::Ready);
    pic.advance_time(Duration::from_nanos(7 * DAY_NS));
    let executed = act(pic, id, owner, "execute", proposed.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    assert_eq!(self::policy(pic, id, owner), policy);
    approver
}

#[test]
fn an_action_waits_for_its_quorum_and_then_expires() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let approver = quorum_of_two(&pic, id, owner);

    let approved = propose(&pic, id, owner, add_member_action(user(9), Role::Viewer)).unwrap();
    assert_eq!(approved.state, ActionState::Waiting);
    assert_eq!(approved.executable_after_ns, 0);
    assert_eq!(
        act(&pic, id, owner, "execute", approved.id),
        Err(WalletError::QuorumNotMet {
            approvals: 1,
            quorum: 2
        })
    );
    let ready = act(&pic, id, approver, "approve", approved.id).unwrap();
    assert_eq!(ready.state, ActionState::Ready);
    assert_eq!(ready.approvals, [owner, approver]);
    assert!(act(&pic, id, approver, "execute", approved.id).is_ok());

    let left = propose(&pic, id, owner, add_member_action(user(10), Role::Viewer)).unwrap();
    assert_eq!(pending(&pic, id, approver), std::slice::from_ref(&left));
    pic.advance_time(Duration::from_nanos(7 * DAY_NS));
    // Queries read the time of the last round.
    pic.tick();
    assert_eq!(pending(&pic, id, approver), []);
    assert!(matches!(
        act(&pic, id, approver, "approve", left.id),
        Err(WalletError::InvalidArgument(reason)) if reason.ends_with("is already expired")
    ));
    assert_eq!(whoami(&pic, id, user(10)), None);
}

#[test]
fn an_approval_stops_counting_when_its_member_is_removed() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let approver = quorum_of_two(&pic, id, owner);
    let second = user(9);
    let added = propose(&pic, id, owner, add_member_action(second, Role::Approver)).unwrap();
    act(&pic, id, approver, "approve", added.id).unwrap();
    act(&pic, id, owner, "execute", added.id).unwrap();

    // The approver backs one action, and the others remove the approver.
    let backed = propose(&pic, id, owner, add_member_action(user(10), Role::Viewer)).unwrap();
    let mut backed = act(&pic, id, approver, "approve", backed.id).unwrap();
    assert_eq!(backed.state, ActionState::Ready);
    let removal = propose(&pic, id, owner, Action::RemoveMember(approver)).unwrap();
    act(&pic, id, second, "approve", removal.id).unwrap();
    pic.advance_time(Duration::from_nanos(24 * HOUR_NS));

    // A join request starts with no approvals, so the approver's is its only
    // one.
    let request = update::<PendingResult>(&pic, id, user(12), "request_join", &"phone").unwrap();
    let request = act(&pic, id, approver, "approve", request.id).unwrap();
    assert_eq!(request.approvals, [approver]);
    assert_eq!(request.state, ActionState::Waiting);

    act(&pic, id, owner, "execute", removal.id).unwrap();

    // A ready action keeps the quorum it reached; a waiting one loses the
    // removed approver's approval.
    let open = pending(&pic, id, owner);
    assert_eq!(open.len(), 2);
    backed.expires_at_ns = open[0].expires_at_ns;
    assert_eq!(open[0], backed);
    assert_eq!(open[1].id, request.id);
    assert_eq!(open[1].approvals, []);
}

#[test]
fn a_join_request_becomes_a_viewer_once_approved() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let phone = user(8);

    let request: PendingResult = update(&pic, id, phone, "request_join", &"phone");
    let request = request.unwrap();
    assert_eq!(request.proposed_by, phone);
    assert_eq!(request.approvals, []);
    assert_eq!(request.state, ActionState::Waiting);
    assert_eq!(request.expires_at_ns, request.proposed_at_ns + HOUR_NS);
    assert_eq!(
        request.action,
        Action::AddMember {
            id: phone,
            role: Role::Viewer,
            label: "phone".to_string()
        }
    );
    let again: PendingResult = update(&pic, id, phone, "request_join", &"other");
    assert_eq!(again.unwrap(), request);
    assert_eq!(pending(&pic, id, owner), std::slice::from_ref(&request));

    act(&pic, id, owner, "approve", request.id).unwrap();
    act(&pic, id, owner, "execute", request.id).unwrap();
    let member = whoami(&pic, id, phone).unwrap();
    assert_eq!(member.role, Role::Viewer);
    assert_eq!(member.added_by, Some(owner));
    assert_eq!(member.label, "phone");
}

/// `request_join` with the role it asks for, as an AI agent's MCP server
/// sends it.
fn request_join(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    label: &str,
    role: Option<Role>,
) -> PendingResult {
    let reply = pic
        .update_call(
            id,
            caller,
            "request_join",
            encode_args((label, role)).unwrap(),
        )
        .unwrap_or_else(|reject| panic!("request_join answers: {reject}"));
    decode_one(&reply).unwrap()
}

#[test]
fn a_join_request_asks_to_be_a_viewer_or_a_proposer_and_nothing_more() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let agent = user(8);

    let request = request_join(&pic, id, agent, "agent", Some(Role::Proposer)).unwrap();
    assert_eq!(request.approvals, []);
    assert_eq!(
        request.action,
        add_member_action_labelled(agent, Role::Proposer, "agent")
    );
    act(&pic, id, owner, "approve", request.id).unwrap();
    act(&pic, id, owner, "execute", request.id).unwrap();
    assert_eq!(whoami(&pic, id, agent).unwrap().role, Role::Proposer);

    let viewer = request_join(&pic, id, user(9), "phone", Some(Role::Viewer)).unwrap();
    assert_eq!(
        viewer.action,
        add_member_action_labelled(user(9), Role::Viewer, "phone")
    );
    for role in [Role::Owner, Role::Approver] {
        assert!(matches!(
            request_join(&pic, id, user(10), "x", Some(role)),
            Err(WalletError::Forbidden(reason)) if reason.contains("a viewer or a proposer")
        ));
    }
}

#[test]
fn join_requests_are_refused_for_members_and_the_anonymous_principal_and_expire() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    for caller in [owner, Principal::anonymous()] {
        let refused: PendingResult = update(&pic, id, caller, "request_join", &"x");
        assert!(
            matches!(refused, Err(WalletError::Forbidden(_))),
            "{refused:?}"
        );
    }
    let empty: PendingResult = update(&pic, id, user(8), "request_join", &" ");
    assert!(matches!(empty, Err(WalletError::InvalidArgument(_))));

    let request: PendingResult = update(&pic, id, user(8), "request_join", &"phone");
    let request = request.unwrap();
    pic.advance_time(Duration::from_nanos(HOUR_NS));
    assert!(matches!(
        act(&pic, id, owner, "approve", request.id),
        Err(WalletError::InvalidArgument(reason)) if reason.ends_with("is already expired")
    ));
    // An expired request does not block a new one.
    let renewed: PendingResult = update(&pic, id, user(8), "request_join", &"phone");
    assert_ne!(renewed.unwrap().id, request.id);
}

#[test]
fn at_most_eight_join_requests_are_open_at_once() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    for seed in 0..8 {
        let request: PendingResult = update(&pic, id, user(100 + seed), "request_join", &"x");
        assert!(request.is_ok());
    }
    let refused: PendingResult = update(&pic, id, user(120), "request_join", &"x");
    assert!(matches!(refused, Err(WalletError::Forbidden(reason)) if reason.contains("at most 8")));
    // Members' proposals are counted apart.
    assert!(propose(&pic, id, owner, add_member_action(user(9), Role::Viewer)).is_ok());
}

#[test]
fn an_owner_rejects_an_action() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let laptop = user(8);
    run_at_once(&pic, id, owner, add_member_action(laptop, Role::Owner));

    let removal = propose(&pic, id, laptop, Action::RemoveMember(owner)).unwrap();
    let rejected = act(&pic, id, owner, "reject", removal.id).unwrap();
    assert_eq!(rejected.state, ActionState::Rejected(owner));
    pic.advance_time(Duration::from_nanos(24 * HOUR_NS));
    assert!(matches!(
        act(&pic, id, laptop, "execute", removal.id),
        Err(WalletError::InvalidArgument(reason)) if reason.ends_with("is already rejected")
    ));
    assert!(whoami(&pic, id, owner).is_some());
}

#[test]
fn an_action_that_no_longer_fits_fails() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let phone = user(8);
    let request: PendingResult = update(&pic, id, phone, "request_join", &"phone");
    let request = act(&pic, id, owner, "approve", request.unwrap().id).unwrap();
    run_at_once(&pic, id, owner, add_member_action(phone, Role::Approver));

    let failed = act(&pic, id, owner, "execute", request.id).unwrap();
    assert!(
        matches!(&failed.state, ActionState::Failed(reason) if reason.contains("already a member")),
        "{failed:?}"
    );
    assert_eq!(whoami(&pic, id, phone).unwrap().role, Role::Approver);
}

#[test]
fn the_last_owner_stays_and_unsupported_actions_are_refused() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    assert!(matches!(
        propose(&pic, id, owner, Action::RemoveMember(owner)),
        Err(WalletError::PolicyViolation(reason)) if reason.contains("at least one owner")
    ));
    let transfer = Action::Transfer(TransferRequest {
        from: 0,
        asset: None,
        to: "someone".to_string(),
        amount: 1u8.into(),
        fee: None,
        memo: None,
    });
    assert_eq!(
        propose(&pic, id, owner, transfer),
        Err(WalletError::NotFound("account 0".to_string()))
    );
    assert!(matches!(
        propose(&pic, id, owner, Action::UninstallExtension(user(3))),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("cannot execute UninstallExtension")
    ));
    assert_eq!(pending(&pic, id, owner), []);
}

#[test]
fn each_role_is_refused_what_it_may_not_do() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let (approver, viewer, proposer) = (user(8), user(9), user(10));
    run_at_once(&pic, id, owner, add_member_action(approver, Role::Approver));
    run_at_once(&pic, id, owner, add_member_action(viewer, Role::Viewer));
    run_at_once(&pic, id, owner, add_member_action(proposer, Role::Proposer));

    let refusals = [
        (approver, "propose", NOT_A_PROPOSER),
        (approver, "reject", NOT_AN_OWNER),
        (approver, "add_account", NOT_AN_OWNER),
        (viewer, "propose", NOT_A_PROPOSER),
        (viewer, "add_account", NOT_AN_OWNER),
        (viewer, "reject", NOT_AN_OWNER),
        (viewer, "approve", NOT_AN_APPROVER),
        (viewer, "execute", NOT_AN_APPROVER),
        (proposer, "add_account", NOT_AN_OWNER),
        (proposer, "reject", NOT_AN_OWNER),
        (proposer, "approve", NOT_AN_APPROVER),
        (proposer, "execute", NOT_AN_APPROVER),
    ];
    for (caller, method, refusal) in refusals {
        let reject = call_without_arguments(&pic, id, caller, method, false).unwrap_err();
        assert_eq!(reject.reject_code, RejectCode::CanisterReject, "{method}");
        assert_eq!(reject.reject_message, refusal, "{method}");
    }
    // Viewers read.
    assert_eq!(pending(&pic, id, viewer), []);
    assert_eq!(accounts(&pic, id, viewer), accounts(&pic, id, owner));
    assert_eq!(policy(&pic, id, viewer), policy(&pic, id, owner));
    assert_eq!(members(&pic, id, viewer).unwrap().len(), 4);
    // Proposers read too.
    assert_eq!(pending(&pic, id, proposer), []);
    assert_eq!(accounts(&pic, id, proposer), accounts(&pic, id, owner));
}

/// The argument of `add_account` before accounts had kinds: one account per
/// chain, ICP and each ICRC-1 ledger apart.
#[derive(CandidType)]
struct ChainAccountArgs {
    chain: Chain,
    label: String,
}

#[derive(CandidType, Deserialize)]
struct AddedAccount {
    id: u32,
}

/// Adds an account the way a wallet without account kinds did; `None` if
/// the wallet has kinds and refuses the argument.
fn add_chain_account(
    pic: &PocketIc,
    id: Principal,
    owner: Principal,
    chain: Chain,
    label: &str,
) -> Option<u32> {
    let args = ChainAccountArgs {
        chain,
        label: label.to_string(),
    };
    let reply = pic
        .update_call(id, owner, "add_account", encode_one(args).unwrap())
        .ok()?;
    let added: Result<AddedAccount, candid::Reserved> = decode_one(&reply).unwrap();
    Some(added.ok().unwrap().id)
}

/// A wallet that the previous release installed, that its owner claimed and
/// that holds open actions, upgrades to this one, keeps its owner and the
/// actions, and runs this release's code. CI
/// builds the previous release from the pull request's base commit (on
/// `main`, from the commit before) and names it in `PREVIOUS_WALLET_WASM`.
/// A change that leaves the Wasm as it was upgrades a wallet to itself.
///
/// A previous release without account kinds kept ICP and each ICRC-1 token
/// in accounts of their own. Its ICP and token accounts with one index are
/// one Internet Computer account after the upgrade, the oldest one's, and
/// its token ledgers are tracked.
#[test]
#[ignore = "needs PREVIOUS_WALLET_WASM, the previous release's Wasm, which CI builds"]
fn a_wallet_from_the_previous_release_upgrades_to_this_one() {
    let path = std::env::var("PREVIOUS_WALLET_WASM")
        .expect("PREVIOUS_WALLET_WASM names the previous release's Wasm");
    let previous = std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let pic = PocketIc::new();
    let id = empty_canister(&pic);
    let arg = encode_one(WalletArg::Init(init_args(&pic))).unwrap();
    pic.install_canister(id, previous, arg, None);
    let owner = user(7);
    claim(&pic, id, owner, claim_signature(id, owner)).unwrap();
    let open = propose(&pic, id, owner, add_member_action(user(8), Role::Viewer)).unwrap();
    let token = Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap();
    let legacy = add_chain_account(&pic, id, owner, Chain::Icp, "main").map(|main| {
        let in_token = add_chain_account(&pic, id, owner, Chain::Icrc(token), "tokens").unwrap();
        let savings = add_chain_account(&pic, id, owner, Chain::Icp, "savings").unwrap();
        assert_eq!((main, in_token, savings), (1, 2, 3));
        // Sent as this release names it: the previous one reads no asset.
        let transfer = Action::Transfer(TransferRequest {
            from: in_token,
            asset: None,
            to: user(9).to_text(),
            amount: 5u8.into(),
            fee: None,
            memo: None,
        });
        propose(&pic, id, owner, transfer).unwrap()
    });

    upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
    assert!(status(&pic, id).claimed);
    assert_eq!(whoami(&pic, id, owner).map(|m| m.role), Some(Role::Owner));
    assert_eq!(members(&pic, id, owner).unwrap().len(), 1);
    let ic = |id, label: &str, index| (id, AccountKind::InternetComputer, label.to_string(), index);
    let listed = || -> Vec<_> {
        accounts(&pic, id, owner)
            .into_iter()
            .map(|account| (account.id, account.kind, account.label, account.index))
            .collect()
    };
    match legacy {
        Some(transfer) => {
            assert_eq!(listed(), [ic(1, "main", 0), ic(3, "savings", 1)]);
            assert_eq!(assets(&pic, id, owner), [Chain::Icp, Chain::Icrc(token)]);
            assert_eq!(status(&pic, id).bitcoin_network, BitcoinNetwork::Mainnet);
            let Action::Transfer(mut request) = transfer.action.clone() else {
                unreachable!()
            };
            (request.from, request.asset) = (1, Some(Chain::Icrc(token)));
            let moved = Pending {
                action: Action::Transfer(request),
                ..transfer
            };
            assert_eq!(pending(&pic, id, owner), [open.clone(), moved]);
            // Indices go on per kind.
            let added = add_account(&pic, id, owner, AccountKind::InternetComputer, "more");
            assert_eq!(added.map(|a| (a.id, a.index)), Ok((4, 2)));
        }
        None => {
            assert_eq!(listed()[0], ic(1, "Main", 0));
            assert_eq!(pending(&pic, id, owner), std::slice::from_ref(&open));
        }
    }
    let executed = act(&pic, id, owner, "execute", open.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    assert_eq!(members(&pic, id, owner).unwrap().len(), 2);
}

// ---- Step 6: the audit log ------------------------------------------------------

fn audit_page(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    before_seq: Option<u64>,
    limit: u16,
) -> Vec<AuditEntry> {
    let request = AuditRequest { before_seq, limit };
    let reply = pic
        .query_call(id, caller, "audit_log", encode_one(request).unwrap())
        .expect("audit_log answers");
    decode_one::<Result<Vec<AuditEntry>, WalletError>>(&reply)
        .unwrap()
        .unwrap()
}

/// The whole log, oldest first, as who did what.
fn audit(pic: &PocketIc, id: Principal, reader: Principal) -> Vec<(Option<Principal>, AuditEvent)> {
    let mut entries = audit_page(pic, id, reader, None, 100);
    entries.reverse();
    for (seq, entry) in entries.iter().enumerate() {
        assert_eq!(entry.seq, seq as u64);
    }
    entries
        .into_iter()
        .map(|entry| (entry.by, entry.event))
        .collect()
}

/// The entries after the first `from`.
fn audit_since(
    pic: &PocketIc,
    id: Principal,
    reader: Principal,
    from: usize,
) -> Vec<(Option<Principal>, AuditEvent)> {
    audit(pic, id, reader).split_off(from)
}

#[test]
fn the_claim_is_the_first_entry() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let entries = audit_page(&pic, id, owner, None, 10);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 0);
    assert_eq!(entries[0].by, Some(owner));
    assert_eq!(entries[0].event, AuditEvent::Claimed);
    assert!(entries[0].at_ns > 0);
}

/// Every supported kind of action, proposed and executed under the default
/// policy, logs exactly its proposal and its execution.
#[test]
fn each_action_kind_logs_its_proposal_and_execution_once() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let laptop = user(8);
    let mut policy = policy(&pic, id, owner);
    policy.rules[0].timelock_ns = HOUR_NS;
    let actions = [
        add_member_action(laptop, Role::Viewer),
        Action::ChangeRole {
            id: laptop,
            role: Role::Approver,
        },
        Action::RemoveMember(laptop),
        Action::SetPolicy(policy),
    ];
    for action in actions {
        let before = audit(&pic, id, owner).len();
        let proposed = propose(&pic, id, owner, action).unwrap();
        // Executing too early is refused and logs nothing.
        if proposed.executable_after_ns > proposed.proposed_at_ns {
            assert!(act(&pic, id, owner, "execute", proposed.id).is_err());
            pic.advance_time(Duration::from_nanos(
                proposed.executable_after_ns - proposed.proposed_at_ns,
            ));
        }
        act(&pic, id, owner, "execute", proposed.id).unwrap();
        assert_eq!(
            audit_since(&pic, id, owner, before),
            [
                (Some(owner), AuditEvent::Proposed(proposed.id)),
                (Some(owner), AuditEvent::Executed(proposed.id)),
            ]
        );
    }
}

#[test]
fn approvals_rejections_and_failures_log_once_and_refusals_never() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let approver = quorum_of_two(&pic, id, owner);
    let before = audit(&pic, id, owner).len();

    let approved = propose(&pic, id, owner, add_member_action(user(9), Role::Viewer)).unwrap();
    act(&pic, id, approver, "approve", approved.id).unwrap();
    // A second approval by the same member changes nothing.
    act(&pic, id, approver, "approve", approved.id).unwrap();
    act(&pic, id, owner, "approve", approved.id).unwrap();
    act(&pic, id, approver, "execute", approved.id).unwrap();
    assert!(act(&pic, id, approver, "execute", approved.id).is_err());

    let rejected = propose(&pic, id, owner, add_member_action(user(10), Role::Viewer)).unwrap();
    act(&pic, id, owner, "reject", rejected.id).unwrap();
    assert!(act(&pic, id, owner, "reject", rejected.id).is_err());
    assert!(act(&pic, id, approver, "approve", rejected.id).is_err());
    assert!(propose(&pic, id, owner, Action::RemoveMember(owner)).is_err());

    assert_eq!(
        audit_since(&pic, id, owner, before),
        [
            (Some(owner), AuditEvent::Proposed(approved.id)),
            (Some(approver), AuditEvent::Approved(approved.id)),
            (Some(approver), AuditEvent::Executed(approved.id)),
            (Some(owner), AuditEvent::Proposed(rejected.id)),
            (Some(owner), AuditEvent::Rejected(rejected.id)),
        ]
    );
}

#[test]
fn a_join_request_logs_who_asked_and_an_expiry_logs_nothing() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let (phone, late) = (user(8), user(9));
    let before = audit(&pic, id, owner).len();

    let request = update::<PendingResult>(&pic, id, phone, "request_join", &"phone").unwrap();
    // Asking again returns the same request and logs nothing.
    update::<PendingResult>(&pic, id, phone, "request_join", &"phone").unwrap();
    act(&pic, id, owner, "approve", request.id).unwrap();
    run_at_once(&pic, id, owner, add_member_action(phone, Role::Approver));
    let failed = act(&pic, id, owner, "execute", request.id).unwrap();
    assert!(matches!(failed.state, ActionState::Failed(_)));

    let expiring = update::<PendingResult>(&pic, id, late, "request_join", &"late").unwrap();
    pic.advance_time(Duration::from_nanos(HOUR_NS));
    assert!(act(&pic, id, owner, "approve", expiring.id).is_err());

    let added = request.id + 1;
    assert_eq!(
        audit_since(&pic, id, owner, before),
        [
            (Some(phone), AuditEvent::JoinRequested(phone)),
            (Some(owner), AuditEvent::Approved(request.id)),
            (Some(owner), AuditEvent::Proposed(added)),
            (Some(owner), AuditEvent::Executed(added)),
            // The execution that failed is logged like any other.
            (Some(owner), AuditEvent::Executed(request.id)),
            (Some(late), AuditEvent::JoinRequested(late)),
        ]
    );
}

#[test]
fn the_audit_log_pages_and_survives_an_upgrade() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    for seed in 0..5 {
        update::<PendingResult>(&pic, id, user(100 + seed), "request_join", &"x").unwrap();
    }
    let all = audit_page(&pic, id, owner, None, 100);
    assert_eq!(all.len(), 6);
    upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
    assert_eq!(audit_page(&pic, id, owner, None, 100), all);

    let seqs = |page: Vec<AuditEntry>| -> Vec<u64> { page.iter().map(|e| e.seq).collect() };
    assert_eq!(seqs(audit_page(&pic, id, owner, None, 2)), [5, 4]);
    assert_eq!(seqs(audit_page(&pic, id, owner, Some(4), 3)), [3, 2, 1]);
    assert_eq!(seqs(audit_page(&pic, id, owner, Some(1), 3)), [0]);
    assert!(audit_page(&pic, id, owner, Some(0), 3).is_empty());
}

#[test]
fn an_audit_page_holds_at_most_a_hundred_entries() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    // The claim and 50 proposals with their executions: 101 entries.
    for seed in 0..50 {
        run_at_once(
            &pic,
            id,
            owner,
            add_member_action(user(100 + seed), Role::Viewer),
        );
    }
    let page = audit_page(&pic, id, owner, None, u16::MAX);
    assert_eq!(page.len(), 100);
    assert_eq!((page[0].seq, page[99].seq), (100, 1));
    assert_eq!(
        audit_page(&pic, id, owner, Some(1), u16::MAX)[0].event,
        AuditEvent::Claimed
    );
}

// ---- Step 7: certified pages -------------------------------------------------

const INDEX_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/index.html");
const SEND_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/send.html");
const APPROVALS_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/approvals.html");
const RECEIVE_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/receive.html");
const ACCOUNT_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/account.html");
const MEMBERS_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/members.html");
const SETTINGS_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/settings.html");
const CLAIM_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/claim.html");
const NOT_FOUND_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/404.html");
const APP_JS: &[u8] = include_bytes!("../../../canisters/wallet/pages/app.js");
const APP_CSS: &[u8] = include_bytes!("../../../canisters/wallet/pages/app.css");

/// The Content-Security-Policy from decision 0001, spelled out here so that
/// any change to it fails this test.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
    img-src 'self' data:; font-src 'self'; \
    connect-src 'self' https://icp-api.io https://icp0.io; frame-src 'none'; \
    frame-ancestors 'none'; object-src 'none'; base-uri 'none'; \
    form-action 'none'; upgrade-insecure-requests";

/// The headers every page, the script and the styles carry, certified.
const PAGE_HEADERS: &[(&str, &str)] = &[
    ("content-security-policy", CSP),
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("referrer-policy", "no-referrer"),
    (
        "strict-transport-security",
        "max-age=31536000; includeSubDomains",
    ),
    ("cache-control", "no-cache"),
];

/// What a gateway allows between the certificate's time and its own clock.
const MAX_CERT_TIME_OFFSET_NS: u128 = 5 * 60 * 1_000_000_000;

/// A wallet on an application subnet of an IC with an NNS subnet, so its
/// certificates carry a delegation from the root key, as on mainnet.
fn wallet_with_root_key() -> (PocketIc, Principal) {
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let subnet = pic.topology().get_app_subnets()[0];
    let id = pic.create_canister_on_subnet(None, None, subnet);
    pic.add_cycles(id, 2_000_000_000_000);
    try_install(&pic, id, &WalletArg::Init(init_args(&pic))).unwrap();
    (pic, id)
}

/// What a gateway sends for `url`, and what the wallet answers, as the types
/// the verifier takes.
fn http_get(
    pic: &PocketIc,
    id: Principal,
    url: &str,
) -> (
    HttpRequest<'static>,
    ic_http_certification::HttpResponse<'static>,
) {
    let request = HttpRequest::get(url)
        .with_headers(vec![("accept".to_string(), "text/html".to_string())])
        .with_certificate_version(2)
        .build();
    let reply = pic
        .query_call(
            id,
            Principal::anonymous(),
            "http_request",
            encode_one(&request).unwrap(),
        )
        .expect("http_request answers");
    let response: wallet::HttpResponse = decode_one(&reply).unwrap();
    let response = ic_http_certification::HttpResponse::builder()
        .with_status_code(StatusCode::from_u16(response.status_code).unwrap())
        .with_headers(response.headers)
        .with_body(response.body.into_vec())
        .build();
    (request, response)
}

/// Verifies a response the way an HTTP gateway does, against the IC's root
/// key and current time, and returns the part that was certified.
fn verify(
    pic: &PocketIc,
    id: Principal,
    request: HttpRequest<'static>,
    response: ic_http_certification::HttpResponse<'static>,
) -> Result<VerifiedResponse, String> {
    let root_key = pic
        .root_key()
        .expect("an IC with an NNS subnet has a root key");
    let now = u128::from(pic.get_time().as_nanos_since_unix_epoch());
    let info = verify_request_response_pair(
        request,
        response,
        id.as_slice(),
        now,
        MAX_CERT_TIME_OFFSET_NS,
        &root_key,
        2,
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(info.verification_version, 2);
    Ok(info
        .response
        .expect("verification v2 returns the certified response"))
}

fn certified_header<'a>(response: &'a VerifiedResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Requests each path and checks that the answer verifies and that its
/// certified status, body and headers are the expected page's.
fn check_every_page(pic: &PocketIc, id: Principal) {
    const HTML: &str = "text/html; charset=utf-8";
    let pages: &[(&str, u16, &[u8], &str)] = &[
        ("/", 200, INDEX_HTML, HTML),
        ("/index.html", 200, INDEX_HTML, HTML),
        ("/?from=link", 200, INDEX_HTML, HTML),
        ("/send", 200, SEND_HTML, HTML),
        ("/send.html", 200, SEND_HTML, HTML),
        ("/receive", 200, RECEIVE_HTML, HTML),
        ("/receive.html", 200, RECEIVE_HTML, HTML),
        ("/account", 200, ACCOUNT_HTML, HTML),
        ("/account.html", 200, ACCOUNT_HTML, HTML),
        ("/approvals", 200, APPROVALS_HTML, HTML),
        ("/approvals.html", 200, APPROVALS_HTML, HTML),
        ("/members", 200, MEMBERS_HTML, HTML),
        ("/members.html", 200, MEMBERS_HTML, HTML),
        ("/settings", 200, SETTINGS_HTML, HTML),
        ("/settings.html", 200, SETTINGS_HTML, HTML),
        ("/claim", 200, CLAIM_HTML, HTML),
        ("/claim.html", 200, CLAIM_HTML, HTML),
        ("/app.js", 200, APP_JS, "text/javascript; charset=utf-8"),
        ("/app.css", 200, APP_CSS, "text/css; charset=utf-8"),
        ("/404.html", 200, NOT_FOUND_HTML, HTML),
        ("/claims", 404, NOT_FOUND_HTML, HTML),
        ("/assets/app.js", 404, NOT_FOUND_HTML, HTML),
    ];
    for &(url, status, body, content_type) in pages {
        let (request, response) = http_get(pic, id, url);
        assert_eq!(response.status_code().as_u16(), status, "{url}");
        assert_eq!(response.body(), body, "{url}");
        let certified = verify(pic, id, request, response).unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(certified.status_code, Some(status), "{url}");
        assert_eq!(certified.body, body, "{url}");
        assert_eq!(
            certified_header(&certified, "content-type"),
            Some(content_type),
            "{url}"
        );
        for &(name, value) in PAGE_HEADERS {
            assert_eq!(
                certified_header(&certified, name),
                Some(value),
                "{url}: {name}"
            );
        }
    }
}

#[test]
fn every_page_verifies_before_and_after_the_claim_and_an_upgrade() {
    let (pic, id) = wallet_with_root_key();
    check_every_page(&pic, id);

    let owner = user(7);
    claim(&pic, id, owner, claim_signature(id, owner)).unwrap();
    check_every_page(&pic, id);

    upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
    check_every_page(&pic, id);
}

/// The verifier these tests use refuses what a gateway must refuse, so the
/// checks above mean something.
#[test]
fn a_changed_page_or_header_fails_verification() {
    let (pic, id) = wallet_with_root_key();
    let (request, response) = http_get(&pic, id, "/");
    verify(&pic, id, request.clone(), response.clone()).unwrap();

    let mut body = response.body().to_vec();
    body.extend_from_slice(b"<script src=/evil.js></script>");
    let changed_body = ic_http_certification::HttpResponse::builder()
        .with_status_code(response.status_code())
        .with_headers(response.headers().to_vec())
        .with_body(body)
        .build();
    assert!(verify(&pic, id, request.clone(), changed_body).is_err());

    let weaker: Vec<_> = response
        .headers()
        .iter()
        .map(|(name, value)| match name.as_str() {
            "content-security-policy" => (name.clone(), "script-src *".to_string()),
            _ => (name.clone(), value.clone()),
        })
        .collect();
    let weaker_csp = ic_http_certification::HttpResponse::builder()
        .with_status_code(response.status_code())
        .with_headers(weaker)
        .with_body(response.body().to_vec())
        .build();
    assert!(verify(&pic, id, request.clone(), weaker_csp).is_err());

    // The 404 page, certified for another path, is not the start page.
    let (_, not_found) = http_get(&pic, id, "/claims");
    assert!(verify(&pic, id, request.clone(), not_found).is_err());

    // Nor is the start page another canister's.
    let other = empty_canister(&pic);
    assert!(verify(&pic, other, request, response).is_err());

    // A URL that cannot be read gets an answer without a certificate.
    let (request, response) = http_get(&pic, id, "/%FF");
    assert_eq!(response.status_code().as_u16(), 400);
    assert!(verify(&pic, id, request, response).is_err());
}

#[test]
fn http_request_answers_only_as_a_query() {
    let (pic, id) = wallet_with_root_key();
    let request = HttpRequest::get("/").build();
    let reject = pic
        .update_call(
            id,
            Principal::anonymous(),
            "http_request",
            encode_one(&request).unwrap(),
        )
        .unwrap_err();
    assert!(
        reject
            .reject_message
            .contains("http_request answers only as a query"),
        "{reject:?}"
    );
}

// ---- Step 8: ICP and ICRC accounts ------------------------------------------

/// The ICP ledger's mainnet id, where the tests install an ICRC-1 ledger.
fn icp_ledger() -> Principal {
    Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap()
}

const ICP_FEE: u64 = 10_000;
const TOKEN_FEE: u64 = 10;
const E8S: u64 = 100_000_000;

/// The ICRC-1 ledger Wasm named by `ICRC1_LEDGER_WASM`, as CI downloads it.
fn ledger_wasm() -> Vec<u8> {
    let path = std::env::var("ICRC1_LEDGER_WASM").expect(
        "ICRC1_LEDGER_WASM names ic-icrc1-ledger.wasm.gz from the dfinity/ic release \
         ledger-suite-icrc-2025-09-01 (sha256 \
         24e6b0b09ba44b1123453877994bb59ce75555c2e33f19f58163a3e0c6e62bd1)",
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[derive(CandidType)]
enum LedgerArg {
    Init(LedgerInit),
}

#[derive(CandidType)]
#[allow(dead_code)]
enum MetadataValue {
    Nat(candid::Nat),
    Int(candid::Int),
    Text(String),
    Blob(ByteBuf),
}

#[derive(CandidType)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType)]
struct ArchiveOptions {
    num_blocks_to_archive: u64,
    max_transactions_per_response: Option<u64>,
    trigger_threshold: u64,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    controller_id: Principal,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType)]
struct LedgerInit {
    minting_account: LedgerAccount,
    fee_collector_account: Option<LedgerAccount>,
    transfer_fee: candid::Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_symbol: String,
    token_name: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(LedgerAccount, candid::Nat)>,
    feature_flags: Option<FeatureFlags>,
    archive_options: ArchiveOptions,
    index_principal: Option<Principal>,
}

/// Subaccount `index` of the wallet, written out here as the mapping the
/// wallet must use: none for 0, else `index` as 32 big-endian bytes.
fn wallet_account(wallet: Principal, index: u32) -> LedgerAccount {
    let subaccount = (index != 0).then(|| {
        let mut bytes = [0; 32];
        bytes[28..].copy_from_slice(&index.to_be_bytes());
        bytes
    });
    LedgerAccount {
        owner: wallet,
        subaccount,
    }
}

fn install_ledger(
    pic: &PocketIc,
    id: Principal,
    symbol: &str,
    fee: u64,
    balances: Vec<(LedgerAccount, u64)>,
) {
    pic.add_cycles(id, 2_000_000_000_000);
    let init = LedgerInit {
        minting_account: LedgerAccount::from(user(100)),
        fee_collector_account: None,
        transfer_fee: fee.into(),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_symbol: symbol.to_string(),
        token_name: symbol.to_string(),
        metadata: vec![],
        initial_balances: balances
            .into_iter()
            .map(|(account, amount)| (account, amount.into()))
            .collect(),
        feature_flags: Some(FeatureFlags { icrc2: false }),
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 1_000,
            max_transactions_per_response: None,
            trigger_threshold: 2_000,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            controller_id: user(100),
            more_controller_ids: None,
        },
        index_principal: None,
    };
    pic.install_canister(
        id,
        ledger_wasm(),
        encode_one(LedgerArg::Init(init)).unwrap(),
        None,
    );
}

/// A claimed wallet on an application subnet, the ICP ledger at its mainnet
/// id on the NNS subnet, and an ICRC token ledger on the wallet's subnet. The
/// wallet's ICP account 0 holds 5 ICP and its token account 0 holds 1,000.
struct Ledgers {
    pic: PocketIc,
    wallet: Principal,
    owner: Principal,
    token: Principal,
}

fn wallet_with_ledgers() -> Ledgers {
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let subnet = pic.topology().get_app_subnets()[0];
    let wallet = pic.create_canister_on_subnet(None, None, subnet);
    pic.add_cycles(wallet, 2_000_000_000_000);
    try_install(&pic, wallet, &WalletArg::Init(init_args(&pic))).unwrap();
    let owner = user(7);
    claim(&pic, wallet, owner, claim_signature(wallet, owner)).unwrap();

    pic.create_canister_with_id(None, None, icp_ledger())
        .unwrap();
    install_ledger(
        &pic,
        icp_ledger(),
        "ICP",
        ICP_FEE,
        vec![(wallet_account(wallet, 0), 5 * E8S)],
    );
    let token = pic.create_canister_on_subnet(None, None, subnet);
    install_ledger(
        &pic,
        token,
        "TKN",
        TOKEN_FEE,
        vec![(wallet_account(wallet, 0), 1_000)],
    );
    Ledgers {
        pic,
        wallet,
        owner,
        token,
    }
}

fn add_account(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    kind: AccountKind,
    label: &str,
) -> Result<Account, WalletError> {
    let args = AddAccountArgs {
        kind,
        label: label.to_string(),
    };
    update(pic, id, caller, "add_account", &args)
}

fn track(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    method: &str,
    asset: Chain,
) -> Result<Vec<Chain>, WalletError> {
    update(pic, id, caller, method, &asset)
}

fn assets(pic: &PocketIc, id: Principal, caller: Principal) -> Vec<Chain> {
    query::<Result<Vec<Chain>, WalletError>>(pic, id, caller, "assets").unwrap()
}

fn accounts(pic: &PocketIc, id: Principal, caller: Principal) -> Vec<Account> {
    query::<Result<Vec<Account>, WalletError>>(pic, id, caller, "accounts").unwrap()
}

fn balance(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    account: u32,
) -> Result<u64, WalletError> {
    balance_of(pic, id, caller, account, None).map(|balance| {
        assert_eq!(balance.account, account);
        u64::try_from(balance.amount.0).unwrap()
    })
}

/// The balance of `asset`, or of the account's default asset.
fn balance_of(
    pic: &PocketIc,
    id: Principal,
    caller: Principal,
    account: u32,
    asset: Option<Chain>,
) -> Result<Balance, WalletError> {
    let reply = pic
        .update_call(
            id,
            caller,
            "balance",
            encode_args((account, asset)).unwrap(),
        )
        .unwrap_or_else(|reject| panic!("balance answers: {reject}"));
    decode_one(&reply).unwrap()
}

/// The balance of `account`, asked of the ledger directly.
fn ledger_balance(pic: &PocketIc, ledger: Principal, account: LedgerAccount) -> u64 {
    let reply = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account).unwrap(),
        )
        .expect("icrc1_balance_of answers");
    u64::try_from(decode_one::<candid::Nat>(&reply).unwrap().0).unwrap()
}

fn transfer_action(from: u32, to: LedgerAccount, amount: u64) -> Action {
    Action::Transfer(TransferRequest {
        from,
        asset: None,
        to: to.to_string(),
        amount: amount.into(),
        fee: None,
        memo: None,
    })
}

fn token_transfer(from: u32, token: Principal, to: LedgerAccount, amount: u64) -> Action {
    Action::Transfer(TransferRequest {
        from,
        asset: Some(Chain::Icrc(token)),
        to: to.to_string(),
        amount: amount.into(),
        fee: None,
        memo: None,
    })
}

/// Executes a ready transfer and returns its ledger block.
fn send(pic: &PocketIc, id: Principal, caller: Principal, action: ActionId) -> u64 {
    let executed = act(pic, id, caller, "execute", action).unwrap();
    let ActionState::Executed(Outcome::Sent(receipt)) = executed.state else {
        panic!("not sent: {executed:?}");
    };
    receipt.tx_id.parse().unwrap()
}

/// Whether `id` reads as an ICP account identifier: 32 bytes in hex. The
/// wallet's unit tests check its value against the ICP ledger's.
fn is_account_id(id: &Option<String>) -> bool {
    id.as_ref().is_some_and(|id| {
        id.len() == 64
            && id
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    })
}

#[test]
fn a_wallet_starts_with_an_internet_computer_account_and_counts_indices_per_kind() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let viewer = user(9);
    run_at_once(&pic, id, owner, add_member_action(viewer, Role::Viewer));

    // The wallet made its Internet Computer account at install. Its Bitcoin
    // account waits for a threshold key, which this IC has none of.
    let listed = accounts(&pic, id, viewer);
    assert!(is_account_id(&listed[0].icp_account_id), "{listed:?}");
    let main = Account {
        id: 1,
        kind: AccountKind::InternetComputer,
        label: "Main".to_string(),
        index: 0,
        holder: AccountHolder::Wallet,
        address: id.to_text(),
        icp_account_id: listed[0].icp_account_id.clone(),
    };
    assert_eq!(listed, std::slice::from_ref(&main));
    let savings = add_account(&pic, id, owner, AccountKind::InternetComputer, "savings").unwrap();
    assert_eq!((savings.id, savings.index), (2, 1));
    assert_eq!(savings.address, wallet_account(id, 1).to_string());
    assert!(is_account_id(&savings.icp_account_id));
    assert_ne!(savings.icp_account_id, main.icp_account_id);

    let refused = [
        (AccountKind::Evm, "eth"),
        (AccountKind::InternetComputer, ""),
    ];
    for (kind, label) in refused {
        assert!(
            matches!(
                add_account(&pic, id, owner, kind, label),
                Err(WalletError::InvalidArgument(_))
            ),
            "{kind:?}"
        );
    }
    // A Bitcoin account asks for a key this IC does not have.
    assert!(matches!(
        add_account(
            &pic,
            id,
            owner,
            AccountKind::Bitcoin(BitcoinNetwork::Regtest),
            "btc"
        ),
        Err(WalletError::ChainError(_))
    ));
    let reject = pic
        .update_call(
            id,
            viewer,
            "add_account",
            encode_one(AddAccountArgs {
                kind: AccountKind::InternetComputer,
                label: "mine".to_string(),
            })
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(reject.reject_message, NOT_AN_OWNER);

    let all = vec![main, savings];
    assert_eq!(accounts(&pic, id, viewer), all);
    upgrade(&pic, id, &WalletArg::Upgrade(None)).unwrap();
    assert_eq!(accounts(&pic, id, owner), all);
    // The log names the accounts owners added, not the default ones.
    let added: Vec<_> = audit(&pic, id, owner)
        .into_iter()
        .filter(|(_, event)| matches!(event, AuditEvent::AccountAdded(_)))
        .collect();
    assert_eq!(added, [(Some(owner), AuditEvent::AccountAdded(2))]);
    assert_eq!(status(&pic, id).bitcoin_network, BitcoinNetwork::Mainnet);
    // With no Bitcoin account yet, an upgrade may set another network.
    let testnet = WalletArg::Upgrade(Some(UpgradeArgs {
        bitcoin_network: Some(BitcoinNetwork::Testnet),
    }));
    upgrade(&pic, id, &testnet).unwrap();
    assert_eq!(status(&pic, id).bitcoin_network, BitcoinNetwork::Testnet);
}

#[test]
fn balance_asks_the_ledger_for_the_accounts_subaccount() {
    let Ledgers {
        pic,
        wallet,
        owner,
        token,
    } = wallet_with_ledgers();
    let viewer = user(9);
    run_at_once(&pic, wallet, owner, add_member_action(viewer, Role::Viewer));
    add_account(
        &pic,
        wallet,
        owner,
        AccountKind::InternetComputer,
        "savings",
    )
    .unwrap();
    let tkn = Chain::Icrc(token);

    assert_eq!(balance(&pic, wallet, viewer, 1), Ok(5 * E8S));
    assert_eq!(balance(&pic, wallet, viewer, 2), Ok(0));
    let icp = balance_of(&pic, wallet, viewer, 1, Some(Chain::Icp)).unwrap();
    assert_eq!((icp.account, icp.asset), (1, Chain::Icp));
    // Only a tracked ledger is asked.
    assert!(matches!(
        balance_of(&pic, wallet, owner, 1, Some(tkn.clone())),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("does not track")
    ));
    assert_eq!(
        track(&pic, wallet, owner, "track_asset", tkn.clone()),
        Ok(vec![Chain::Icp, tkn.clone()])
    );
    let held = balance_of(&pic, wallet, viewer, 1, Some(tkn.clone())).unwrap();
    assert_eq!(
        (held.account, held.asset, held.amount),
        (1, tkn.clone(), 1_000u32.into())
    );
    assert!(matches!(
        balance_of(&pic, wallet, owner, 1, Some(REGTEST)),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("does not hold")
    ));
    assert_eq!(
        balance(&pic, wallet, owner, 4),
        Err(WalletError::NotFound("account 4".to_string()))
    );

    pic.stop_canister(token, None).unwrap();
    assert!(matches!(
        balance_of(&pic, wallet, owner, 1, Some(tkn.clone())),
        Err(WalletError::ChainError(reason)) if reason.contains("icrc1_balance_of")
    ));
    pic.start_canister(token, None).unwrap();
    assert_eq!(
        balance_of(&pic, wallet, owner, 1, Some(tkn.clone())).map(|b| b.amount),
        Ok(1_000u32.into())
    );
}

#[test]
fn owners_track_icrc_ledgers_that_answer_as_one() {
    let Ledgers {
        pic,
        wallet,
        owner,
        token,
    } = wallet_with_ledgers();
    let viewer = user(9);
    run_at_once(&pic, wallet, owner, add_member_action(viewer, Role::Viewer));
    let tkn = Chain::Icrc(token);
    assert_eq!(assets(&pic, wallet, viewer), [Chain::Icp]);

    // A canister that is no ledger, and the assets that are not tracked.
    let not_a_ledger = empty_canister(&pic);
    assert!(matches!(
        track(
            &pic,
            wallet,
            owner,
            "track_asset",
            Chain::Icrc(not_a_ledger)
        ),
        Err(WalletError::ChainError(_))
    ));
    for asset in [
        Chain::Icp,
        Chain::Icrc(icp_ledger()),
        REGTEST,
        Chain::Evm(1),
    ] {
        assert!(
            matches!(
                track(&pic, wallet, owner, "track_asset", asset.clone()),
                Err(WalletError::InvalidArgument(_))
            ),
            "{asset:?}"
        );
    }
    let reject = pic
        .update_call(
            wallet,
            viewer,
            "track_asset",
            encode_one(tkn.clone()).unwrap(),
        )
        .unwrap_err();
    assert_eq!(reject.reject_message, NOT_AN_OWNER);

    track(&pic, wallet, owner, "track_asset", tkn.clone()).unwrap();
    assert!(track(&pic, wallet, owner, "track_asset", tkn.clone()).is_err());
    upgrade(&pic, wallet, &WalletArg::Upgrade(None)).unwrap();
    assert_eq!(assets(&pic, wallet, viewer), [Chain::Icp, tkn.clone()]);

    // An open transfer of a token no longer tracked still runs.
    let friend = LedgerAccount::from(user(20));
    let open = propose(&pic, wallet, owner, token_transfer(1, token, friend, 10)).unwrap();
    assert_eq!(
        track(&pic, wallet, owner, "untrack_asset", tkn.clone()),
        Ok(vec![Chain::Icp])
    );
    assert!(matches!(
        track(&pic, wallet, owner, "untrack_asset", Chain::Icp),
        Err(WalletError::InvalidArgument(_))
    ));
    assert!(matches!(
        propose(&pic, wallet, owner, token_transfer(1, token, friend, 10)),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("does not track")
    ));
    send(&pic, wallet, owner, open.id);
    assert_eq!(ledger_balance(&pic, token, friend), 10);
    let log = audit(&pic, wallet, owner);
    assert!(log.contains(&(Some(owner), AuditEvent::AssetTracked(tkn.clone()))));
    assert!(log.contains(&(Some(owner), AuditEvent::AssetUntracked(tkn))));
}

#[test]
fn a_transfer_sends_from_its_accounts_subaccount_once() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    let savings = add_account(
        &pic,
        wallet,
        owner,
        AccountKind::InternetComputer,
        "savings",
    )
    .unwrap();
    let friend = LedgerAccount::from(user(20));
    let before = audit(&pic, wallet, owner).len();

    // From account 0 to the wallet's own subaccount 1, with a memo.
    let to_savings = Action::Transfer(TransferRequest {
        from: 1,
        asset: None,
        to: savings.address.clone(),
        amount: (2 * E8S).into(),
        fee: Some(ICP_FEE.into()),
        memo: Some(ByteBuf::from(b"to savings".to_vec())),
    });
    let proposed = propose(&pic, wallet, owner, to_savings).unwrap();
    assert_eq!(proposed.state, ActionState::Ready);
    let executed = act(&pic, wallet, owner, "execute", proposed.id).unwrap();
    let ActionState::Executed(Outcome::Sent(receipt)) = &executed.state else {
        panic!("not sent: {executed:?}");
    };
    assert_eq!((receipt.account, &receipt.chain), (1, &Chain::Icp));
    // Block 0 holds the initial balance.
    assert_eq!(receipt.tx_id, "1");
    assert_eq!(
        ledger_balance(&pic, icp_ledger(), wallet_account(wallet, 1)),
        2 * E8S
    );
    assert_eq!(
        ledger_balance(&pic, icp_ledger(), wallet_account(wallet, 0)),
        3 * E8S - ICP_FEE
    );
    assert!(matches!(
        act(&pic, wallet, owner, "execute", proposed.id),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("already executed")
    ));

    // From subaccount 1 to someone else.
    let proposed = propose(&pic, wallet, owner, transfer_action(2, friend, E8S)).unwrap();
    assert_eq!(send(&pic, wallet, owner, proposed.id), 2);
    assert_eq!(ledger_balance(&pic, icp_ledger(), friend), E8S);
    assert_eq!(balance(&pic, wallet, owner, 2), Ok(E8S - ICP_FEE));
    assert_eq!(
        audit_since(&pic, wallet, owner, before),
        [
            (Some(owner), AuditEvent::Proposed(1)),
            (Some(owner), AuditEvent::Executed(1)),
            (Some(owner), AuditEvent::Proposed(2)),
            (Some(owner), AuditEvent::Executed(2)),
        ]
    );
}

#[test]
fn propose_checks_a_transfer() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let friend = LedgerAccount::from(user(20)).to_string();
    let request = |from, to: &str, amount: u64, memo: usize| {
        Action::Transfer(TransferRequest {
            from,
            asset: None,
            to: to.to_string(),
            amount: amount.into(),
            fee: None,
            memo: Some(ByteBuf::from(vec![1; memo])),
        })
    };
    assert_eq!(
        propose(&pic, id, owner, request(2, &friend, 1, 0)),
        Err(WalletError::NotFound("account 2".to_string()))
    );
    for action in [
        request(1, "someone", 1, 0),
        request(1, &Principal::anonymous().to_text(), 1, 0),
        request(1, &friend, 0, 0),
        request(1, &friend, 1, 33),
    ] {
        assert!(
            matches!(
                propose(&pic, id, owner, action.clone()),
                Err(WalletError::InvalidArgument(_))
            ),
            "{action:?}"
        );
    }
    assert!(propose(&pic, id, owner, request(1, &friend, 1, 32)).is_ok());
}

#[test]
fn a_transfer_to_a_stopped_ledger_stays_ready_and_is_sent_once_it_runs() {
    let Ledgers {
        pic,
        wallet,
        owner,
        token,
    } = wallet_with_ledgers();
    track(&pic, wallet, owner, "track_asset", Chain::Icrc(token)).unwrap();
    let friend = LedgerAccount::from(user(20));
    let proposed = propose(&pic, wallet, owner, token_transfer(1, token, friend, 100)).unwrap();

    pic.stop_canister(token, None).unwrap();
    assert!(matches!(
        act(&pic, wallet, owner, "execute", proposed.id),
        Err(WalletError::ChainError(reason)) if reason.contains("icrc1_transfer")
    ));
    assert_eq!(pending(&pic, wallet, owner)[0].state, ActionState::Ready);

    pic.start_canister(token, None).unwrap();
    send(&pic, wallet, owner, proposed.id);
    assert_eq!(ledger_balance(&pic, token, friend), 100);
    assert_eq!(
        ledger_balance(&pic, token, wallet_account(wallet, 0)),
        1_000 - 100 - TOKEN_FEE
    );
}

#[test]
fn the_wallet_finds_out_by_itself_whether_a_transfer_it_could_not_tell_was_sent() {
    let Ledgers {
        pic,
        wallet,
        owner,
        token,
    } = wallet_with_ledgers();
    track(&pic, wallet, owner, "track_asset", Chain::Icrc(token)).unwrap();
    let friend = LedgerAccount::from(user(20));
    let proposed = propose(&pic, wallet, owner, token_transfer(1, token, friend, 100)).unwrap();
    pic.stop_canister(token, None).unwrap();
    assert!(act(&pic, wallet, owner, "execute", proposed.id).is_err());
    pic.start_canister(token, None).unwrap();

    // The wallet's timer makes the call again ten minutes later, with the
    // same `created_at_time`, so the ledger sends the tokens at most once.
    pic.advance_time(Duration::from_secs(10 * 60));
    for _ in 0..10 {
        pic.tick();
    }
    assert_eq!(pending(&pic, wallet, owner), []);
    assert_eq!(
        audit(&pic, wallet, owner).last(),
        Some(&(None, AuditEvent::Executed(proposed.id)))
    );
    assert_eq!(ledger_balance(&pic, token, friend), 100);
    assert!(matches!(
        act(&pic, wallet, owner, "execute", proposed.id),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("already executed")
    ));
}

#[test]
fn a_second_execute_while_the_first_is_in_flight_is_refused() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    let friend = LedgerAccount::from(user(20));
    let proposed = propose(&pic, wallet, owner, transfer_action(1, friend, E8S)).unwrap();

    let execute = || {
        pic.submit_call(wallet, owner, "execute", encode_one(proposed.id).unwrap())
            .unwrap()
    };
    let (first, second) = (execute(), execute());
    let decode = |reply: Vec<u8>| decode_one::<PendingResult>(&reply).unwrap();
    let mut results = [
        decode(pic.await_call(first).unwrap()),
        decode(pic.await_call(second).unwrap()),
    ];
    // Either message may run first.
    results.sort_by_key(|result| result.is_err());
    assert!(
        matches!(&results[0], Ok(pending) if matches!(pending.state, ActionState::Executed(Outcome::Sent(_)))),
        "{results:?}"
    );
    assert!(
        matches!(&results[1], Err(WalletError::InvalidArgument(reason)) if reason.contains("in flight")),
        "{results:?}"
    );
    assert_eq!(ledger_balance(&pic, icp_ledger(), friend), E8S);
}

/// Two actions that send the same amount to the same account, executed in
/// one round, are two transfers: each ledger call has its own
/// `created_at_time`, so the ledger takes neither for a repeat of the other.
#[test]
fn identical_transfers_executed_together_are_both_sent() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    let friend = LedgerAccount::from(user(20));
    let first = propose(&pic, wallet, owner, transfer_action(1, friend, E8S)).unwrap();
    let second = propose(&pic, wallet, owner, transfer_action(1, friend, E8S)).unwrap();

    let execute = |action: ActionId| {
        pic.submit_call(wallet, owner, "execute", encode_one(action).unwrap())
            .unwrap()
    };
    let calls = [execute(first.id), execute(second.id)];
    let blocks: Vec<String> = calls
        .into_iter()
        .map(|call| {
            let result = decode_one::<PendingResult>(&pic.await_call(call).unwrap()).unwrap();
            match result.map(|pending| pending.state) {
                Ok(ActionState::Executed(Outcome::Sent(receipt))) => receipt.tx_id,
                other => panic!("not sent: {other:?}"),
            }
        })
        .collect();
    assert_ne!(blocks[0], blocks[1]);
    assert_eq!(ledger_balance(&pic, icp_ledger(), friend), 2 * E8S);
}

#[test]
fn a_transfer_the_ledger_refuses_fails() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    let friend = LedgerAccount::from(user(20));

    let too_much = propose(&pic, wallet, owner, transfer_action(1, friend, 5 * E8S)).unwrap();
    let failed = act(&pic, wallet, owner, "execute", too_much.id).unwrap();
    assert_eq!(
        failed.state,
        ActionState::Failed(format!(
            "the account holds {}, less than the amount and the fee",
            5 * E8S
        ))
    );

    let wrong_fee = Action::Transfer(TransferRequest {
        from: 1,
        asset: None,
        to: friend.to_string(),
        amount: E8S.into(),
        fee: Some(1u8.into()),
        memo: None,
    });
    let wrong_fee = propose(&pic, wallet, owner, wrong_fee).unwrap();
    let failed = act(&pic, wallet, owner, "execute", wrong_fee.id).unwrap();
    assert_eq!(
        failed.state,
        ActionState::Failed(format!("the ledger's fee is {ICP_FEE}"))
    );
    assert_eq!(balance(&pic, wallet, owner, 1), Ok(5 * E8S));
    let entries = audit(&pic, wallet, owner);
    assert_eq!(
        entries[entries.len() - 2..],
        [
            (Some(owner), AuditEvent::Proposed(wrong_fee.id)),
            (Some(owner), AuditEvent::Executed(wrong_fee.id)),
        ]
    );
}

/// Sets a Transfer rule of one approval and a day's timelock, with a daily
/// limit of 2 ICP, which takes the SetPolicy timelock of seven days.
fn icp_daily_limit(pic: &PocketIc, id: Principal, owner: Principal) {
    let mut policy = policy(pic, id, owner);
    let rule = policy
        .rules
        .iter_mut()
        .find(|rule| rule.kind == ActionKind::Transfer)
        .unwrap();
    rule.timelock_ns = DAY_NS;
    rule.daily_limits = vec![DailyLimit {
        chain: Chain::Icp,
        amount: (2 * E8S).into(),
    }];
    let proposed = propose(pic, id, owner, Action::SetPolicy(policy)).unwrap();
    pic.advance_time(Duration::from_nanos(7 * DAY_NS));
    act(pic, id, owner, "execute", proposed.id).unwrap();
}

#[test]
fn the_daily_limit_releases_small_transfers_at_once() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    icp_daily_limit(&pic, wallet, owner);
    let friend = LedgerAccount::from(user(20));
    let at_once = |pending: &Pending| pending.executable_after_ns == pending.proposed_at_ns;

    let first = propose(&pic, wallet, owner, transfer_action(1, friend, 3 * E8S / 2)).unwrap();
    assert!(at_once(&first));
    send(&pic, wallet, owner, first.id);
    // 1.5 sent and 0.4 open stay within 2 ICP; 0.2 more would not.
    let second = propose(&pic, wallet, owner, transfer_action(1, friend, 2 * E8S / 5)).unwrap();
    assert!(at_once(&second));
    let third = propose(&pic, wallet, owner, transfer_action(1, friend, E8S / 5)).unwrap();
    assert_eq!(third.executable_after_ns, third.proposed_at_ns + DAY_NS);
    assert!(matches!(
        act(&pic, wallet, owner, "execute", third.id),
        Err(WalletError::TimelockActive { .. })
    ));
    send(&pic, wallet, owner, second.id);

    // A day later the first two no longer count, and the third does.
    pic.advance_time(Duration::from_nanos(DAY_NS + 1));
    send(&pic, wallet, owner, third.id);
    let fourth = propose(&pic, wallet, owner, transfer_action(1, friend, 3 * E8S / 2)).unwrap();
    assert!(at_once(&fourth));
    let fifth = propose(&pic, wallet, owner, transfer_action(1, friend, 2 * E8S / 5)).unwrap();
    assert!(!at_once(&fifth));
    send(&pic, wallet, owner, fourth.id);
    assert_eq!(
        ledger_balance(&pic, icp_ledger(), friend),
        3 * E8S / 2 + 2 * E8S / 5 + E8S / 5 + 3 * E8S / 2
    );
}

/// A proposer's transfer waits for an owner even under the daily limit,
/// which would release an owner's at once, and a proposer proposes nothing
/// but transfers, at most eight open at once.
#[test]
fn a_proposers_transfer_waits_for_an_owner() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    icp_daily_limit(&pic, wallet, owner);
    let agent = user(30);
    run_at_once(
        &pic,
        wallet,
        owner,
        add_member_action(agent, Role::Proposer),
    );
    let friend = LedgerAccount::from(user(20));
    let before = audit(&pic, wallet, owner).len();

    let proposed = propose(&pic, wallet, agent, transfer_action(1, friend, E8S)).unwrap();
    assert_eq!(proposed.proposed_by, agent);
    assert_eq!(proposed.approvals, []);
    assert_eq!(proposed.state, ActionState::Waiting);
    assert_eq!(
        act(&pic, wallet, owner, "execute", proposed.id),
        Err(WalletError::QuorumNotMet {
            approvals: 0,
            quorum: 1
        })
    );
    let approved = act(&pic, wallet, owner, "approve", proposed.id).unwrap();
    assert_eq!(approved.state, ActionState::Ready);
    send(&pic, wallet, owner, proposed.id);
    assert_eq!(ledger_balance(&pic, icp_ledger(), friend), E8S);
    assert_eq!(
        audit_since(&pic, wallet, owner, before),
        [
            (Some(agent), AuditEvent::Proposed(proposed.id)),
            (Some(owner), AuditEvent::Approved(proposed.id)),
            (Some(owner), AuditEvent::Executed(proposed.id)),
        ]
    );

    for action in [
        add_member_action(user(31), Role::Viewer),
        Action::RemoveMember(owner),
        Action::ChangeRole {
            id: agent,
            role: Role::Owner,
        },
        Action::SetPolicy(policy(&pic, wallet, owner)),
    ] {
        assert_eq!(
            propose(&pic, wallet, agent, action),
            Err(WalletError::Forbidden(
                "a proposer proposes only transfers".to_string()
            ))
        );
    }
    for _ in 0..8 {
        propose(&pic, wallet, agent, transfer_action(1, friend, 1)).unwrap();
    }
    assert!(matches!(
        propose(&pic, wallet, agent, transfer_action(1, friend, 1)),
        Err(WalletError::Forbidden(reason)) if reason.contains("at most 8")
    ));
    // The owner still proposes.
    propose(&pic, wallet, owner, transfer_action(1, friend, 1)).unwrap();
}

#[test]
fn a_transfer_under_the_daily_limit_is_checked_again_when_it_runs() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    icp_daily_limit(&pic, wallet, owner);
    let mut lower = policy(&pic, wallet, owner);
    for rule in &mut lower.rules {
        for limit in &mut rule.daily_limits {
            limit.amount = (E8S / 2).into();
        }
    }
    let set_policy = propose(&pic, wallet, owner, Action::SetPolicy(lower)).unwrap();
    pic.advance_time(Duration::from_nanos(7 * DAY_NS - HOUR_NS));
    let friend = LedgerAccount::from(user(20));
    let transfer = propose(&pic, wallet, owner, transfer_action(1, friend, E8S)).unwrap();
    assert_eq!(transfer.executable_after_ns, transfer.proposed_at_ns);

    pic.advance_time(Duration::from_nanos(HOUR_NS));
    act(&pic, wallet, owner, "execute", set_policy.id).unwrap();
    let failed = act(&pic, wallet, owner, "execute", transfer.id).unwrap();
    assert!(
        matches!(&failed.state, ActionState::Failed(reason) if reason.contains("no longer fits")),
        "{failed:?}"
    );
    assert_eq!(ledger_balance(&pic, icp_ledger(), friend), 0);
}

// ---- Step 9: Bitcoin on regtest --------------------------------------------------

const BTC: u64 = 100_000_000;
const REGTEST: Chain = Chain::Bitcoin(BitcoinNetwork::Regtest);
/// BIP-173's example regtest address, whose key nobody here holds.
const BURN: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

/// The Bitcoin canister of every network but mainnet.
fn bitcoin_canister() -> Principal {
    Principal::from_text("g4xu7-jiaaa-aaaan-aaaaq-cai").unwrap()
}

/// One regtest network at a time: each test runs `bitcoind` and a PocketIC
/// instance with four subnets.
static REGTEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A regtest node of its own, from the binaries `BITCOIND` and `BITCOIN_CLI`
/// name (Bitcoin Core 31.1 in CI), stopped and deleted on drop.
struct Bitcoind {
    process: Child,
    p2p: u16,
    rpc: u16,
    dir: PathBuf,
}

impl Bitcoind {
    fn start() -> Bitcoind {
        let (p2p, rpc) = (free_port(), free_port());
        let dir = std::env::temp_dir().join(format!("wallet-regtest-{p2p}-{rpc}"));
        std::fs::create_dir_all(&dir).unwrap();
        let process = Bitcoind::spawn(p2p, rpc, &dir, &[]);
        let node = Bitcoind {
            process,
            p2p,
            rpc,
            dir,
        };
        node.wait_until_up();
        node
    }

    fn spawn(p2p: u16, rpc: u16, dir: &Path, extra: &[&str]) -> Child {
        let binary = std::env::var("BITCOIND").expect("BITCOIND names bitcoind");
        Command::new(binary)
            .args([
                "-regtest",
                "-listen=1",
                "-bind=127.0.0.1",
                &format!("-port={p2p}"),
                &format!("-rpcport={rpc}"),
                "-rpcuser=wallet",
                "-rpcpassword=wallet",
                &format!("-datadir={}", dir.display()),
                "-server=1",
                "-txindex=1",
                "-printtoconsole=0",
            ])
            .args(extra)
            .stdout(Stdio::null())
            .spawn()
            .expect("bitcoind starts")
    }

    fn wait_until_up(&self) {
        let start = Instant::now();
        while self.cli(&["getblockchaininfo"]).is_err() {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "bitcoind did not answer"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stops the node and starts it again on its ports and blocks, with the
    /// `extra` arguments.
    fn restart(&mut self, extra: &[&str]) {
        self.call(&["stop"]);
        self.process.wait().unwrap();
        self.process = Bitcoind::spawn(self.p2p, self.rpc, &self.dir, extra);
        self.wait_until_up();
    }

    fn cli(&self, args: &[&str]) -> Result<String, String> {
        let binary = std::env::var("BITCOIN_CLI").expect("BITCOIN_CLI names bitcoin-cli");
        let output = Command::new(binary)
            .args([
                "-regtest",
                &format!("-rpcport={}", self.rpc),
                "-rpcuser=wallet",
                "-rpcpassword=wallet",
            ])
            .args(args)
            .output()
            .unwrap();
        if output.status.success() {
            Ok(String::from_utf8(output.stdout).unwrap().trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    fn call(&self, args: &[&str]) -> String {
        self.cli(args)
            .unwrap_or_else(|error| panic!("bitcoin-cli {args:?}: {error}"))
    }

    /// A transaction in the mempool or a block.
    fn transaction(&self, txid: &str) -> bitcoin::Transaction {
        let hex = self.call(&["getrawtransaction", txid]);
        bitcoin::consensus::encode::deserialize_hex(&hex).unwrap()
    }

    fn mempool(&self) -> String {
        self.call(&["getrawmempool"])
    }
}

impl Drop for Bitcoind {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A claimed wallet next to a regtest network that the Bitcoin canister
/// follows.
struct Regtest {
    pic: PocketIc,
    node: Bitcoind,
    wallet: Principal,
    owner: Principal,
    _lock: std::sync::MutexGuard<'static, ()>,
}

fn wallet_on_regtest() -> Regtest {
    wallet_on_regtest_after(|_, _, _| ())
}

/// A wallet on regtest, after `prepare` ran on it while the IC's clock was
/// still before the blocks' and could run ahead.
fn wallet_on_regtest_after(prepare: impl FnOnce(&PocketIc, Principal, Principal)) -> Regtest {
    let lock = REGTEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let node = Bitcoind::start();
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_test_threshold_keys_subnet()
        .with_bitcoin_subnet()
        .with_application_subnet()
        .with_bitcoind_addr(SocketAddr::from(([127, 0, 0, 1], node.p2p)))
        .with_icp_features(IcpFeatures {
            bitcoin: Some(IcpFeaturesConfig::DefaultConfig),
            ..Default::default()
        })
        .build();
    let subnet = pic.topology().get_app_subnets()[0];
    let wallet = pic.create_canister_on_subnet(None, None, subnet);
    pic.add_cycles(wallet, 100_000_000_000_000);
    let args = InitArgs {
        bitcoin_network: Some(BitcoinNetwork::Regtest),
        ..init_args(&pic)
    };
    try_install(&pic, wallet, &WalletArg::Init(args)).unwrap();
    let owner = user(7);
    claim(&pic, wallet, owner, claim_signature(wallet, owner)).unwrap();
    prepare(&pic, wallet, owner);
    // The Bitcoin canister follows blocks only near the IC's time.
    let now = SystemTime::now().into();
    assert!(pic.get_time() < now, "the IC's clock ran past the blocks'");
    pic.set_time(now);
    Regtest {
        pic,
        node,
        wallet,
        owner,
        _lock: lock,
    }
}

impl Regtest {
    /// Mines `blocks` to `to`, and waits until the Bitcoin canister has them.
    fn mine(&self, blocks: u32, to: &str) {
        self.node
            .call(&["generatetoaddress", &blocks.to_string(), to]);
        let height: u32 = self.node.call(&["getblockcount"]).parse().unwrap();
        let start = Instant::now();
        while self.tip_height() < Some(height) {
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "the Bitcoin canister did not reach height {height}"
            );
            self.pic.tick();
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The Bitcoin canister's tip; `None` while it is not synced.
    fn tip_height(&self) -> Option<u32> {
        let request = GetUtxosRequest {
            address: BURN.to_string(),
            network: NetworkInRequest::Regtest,
            filter: None,
        };
        self.pic
            .query_call(
                bitcoin_canister(),
                Principal::anonymous(),
                "bitcoin_get_utxos_query",
                encode_one(request).unwrap(),
            )
            .ok()
            .and_then(|reply| decode_one::<GetUtxosResponse>(&reply).ok())
            .map(|response| response.tip_height)
    }

    fn add_bitcoin_account(&self, label: &str) -> Account {
        let kind = AccountKind::Bitcoin(BitcoinNetwork::Regtest);
        add_account(&self.pic, self.wallet, self.owner, kind, label).unwrap()
    }

    /// The Bitcoin account the wallet makes itself, once the threshold key
    /// subnet gave it its key.
    fn main_account(&self) -> Account {
        let start = Instant::now();
        loop {
            let found = accounts(&self.pic, self.wallet, self.owner)
                .into_iter()
                .find(|account| account.kind == AccountKind::Bitcoin(BitcoinNetwork::Regtest));
            if let Some(account) = found {
                return account;
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "the wallet made no Bitcoin account"
            );
            self.pic.tick();
        }
    }

    /// Lets the wallet's timer run, and any calls it makes end.
    fn run_timer(&self) {
        for _ in 0..50 {
            self.pic.tick();
        }
    }

    /// How many transactions the Bitcoin canister was handed, from its
    /// metrics.
    fn sends(&self) -> u64 {
        let request = HttpRequest::get("/metrics").build();
        let reply = self
            .pic
            .query_call(
                bitcoin_canister(),
                Principal::anonymous(),
                "http_request",
                encode_one(&request).unwrap(),
            )
            .unwrap();
        let response: wallet::HttpResponse = decode_one(&reply).unwrap();
        let metrics = String::from_utf8(response.body.into_vec()).unwrap();
        let line = metrics
            .lines()
            .find_map(|line| line.strip_prefix("send_transaction_count "))
            .expect("the Bitcoin canister counts its sends");
        line.split(' ').next().unwrap().parse().unwrap()
    }

    /// Runs the wallet's timer until `done`, ticking the IC.
    fn tick_until(&self, what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(120), "{what}");
            self.pic.tick();
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn propose(&self, action: Action) -> PendingResult {
        propose(&self.pic, self.wallet, self.owner, action)
    }

    fn execute(&self, action: ActionId) -> PendingResult {
        act(&self.pic, self.wallet, self.owner, "execute", action)
    }

    /// Executes a ready transfer and returns its transaction.
    fn send(&self, action: ActionId) -> bitcoin::Transaction {
        let txid = self.send_txid(action);
        let tx = self.node.transaction(&txid);
        assert_eq!(tx.compute_txid().to_string(), txid);
        tx
    }

    /// The transactions in the node's mempool.
    fn mempool(&self) -> Vec<String> {
        self.node
            .mempool()
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    fn send_txid(&self, action: ActionId) -> String {
        let executed = self.execute(action).unwrap();
        let ActionState::Executed(Outcome::Sent(receipt)) = executed.state else {
            panic!("not sent: {executed:?}");
        };
        assert_eq!(receipt.chain, REGTEST);
        self.wait_for_mempool(&receipt.tx_id);
        receipt.tx_id
    }

    /// Waits until the Bitcoin canister's adapter has passed a transaction
    /// on to the node.
    fn wait_for_mempool(&self, txid: &str) {
        let start = Instant::now();
        while !self.node.mempool().contains(txid) {
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "{txid} did not reach the mempool"
            );
            self.pic.tick();
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn balance(&self, account: u32) -> u64 {
        balance(&self.pic, self.wallet, self.owner, account).unwrap()
    }

    fn pending(&self, action: ActionId) -> Pending {
        pending(&self.pic, self.wallet, self.owner)
            .into_iter()
            .find(|pending| pending.id == action)
            .unwrap()
    }

    /// Sets what the Bitcoin canister charges, as its controller.
    fn set_bitcoin_fees(&self, fees: Fees) {
        let controller = self.pic.get_controllers(bitcoin_canister())[0];
        let request = SetConfigRequest {
            fees: Some(fees),
            ..Default::default()
        };
        update::<()>(
            &self.pic,
            bitcoin_canister(),
            controller,
            "set_config",
            &request,
        );
    }

    fn bitcoin_fees(&self) -> Fees {
        query::<Config>(
            &self.pic,
            bitcoin_canister(),
            Principal::anonymous(),
            "get_config",
        )
        .fees
    }
}

fn bitcoin_transfer(from: u32, to: &str, amount: u64, fee: Option<u64>) -> Action {
    Action::Transfer(TransferRequest {
        from,
        asset: None,
        to: to.to_string(),
        amount: amount.into(),
        fee: fee.map(Into::into),
        memo: None,
    })
}

/// The address an output pays.
fn paid_to(output: &bitcoin::TxOut) -> String {
    bitcoin::Address::from_script(&output.script_pubkey, bitcoin::Network::Regtest)
        .unwrap()
        .to_string()
}

fn outputs(tx: &bitcoin::Transaction) -> Vec<(String, u64)> {
    tx.output
        .iter()
        .map(|output| (paid_to(output), output.value.to_sat()))
        .collect()
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn a_bitcoin_account_receives_at_its_keys_address_and_sends_once() {
    let net = wallet_on_regtest();
    let main = net.main_account();
    let savings = net.add_bitcoin_account("savings");
    // Account 1 is the Internet Computer account the wallet starts with.
    let ic = accounts(&net.pic, net.wallet, net.owner).remove(0);
    assert_eq!((ic.id, ic.kind), (1, AccountKind::InternetComputer));
    assert_eq!((main.id, main.index, main.label.as_str()), (2, 0, "Main"));
    assert_eq!((savings.id, savings.index), (3, 1));
    assert_ne!(main.address, savings.address);
    for account in [&main, &savings] {
        // A P2WPKH address: a version 0 program of 20 bytes.
        let info = net.node.call(&["validateaddress", &account.address]);
        assert!(info.contains("\"isvalid\": true"), "{info}");
        assert!(info.contains("\"witness_version\": 0"), "{info}");
        assert!(account.address.starts_with("bcrt1q") && account.address.len() == 44);
    }

    // A coinbase output can be spent 100 blocks later.
    net.mine(1, &main.address);
    net.mine(100, BURN);
    assert_eq!(net.balance(main.id), 50 * BTC);
    assert_eq!(net.balance(savings.id), 0);

    let proposed = net
        .propose(bitcoin_transfer(main.id, &savings.address, BTC, None))
        .unwrap();
    assert_eq!(proposed.state, ActionState::Ready);
    let tx = net.send(proposed.id);
    // The network checked every signature; the transaction pays the
    // recipient first and the change back to the account.
    assert_eq!(tx.input.len(), 1);
    let change = outputs(&tx)[1].1;
    assert_eq!(
        outputs(&tx),
        [
            (savings.address.clone(), BTC),
            (main.address.clone(), change)
        ]
    );
    // Two satoshi per vbyte: the rate without fee history.
    let fee = 50 * BTC - BTC - change;
    assert!(
        fee >= 2 * tx.vsize() as u64 && fee < 3 * tx.vsize() as u64,
        "{fee}"
    );
    assert!(matches!(
        net.execute(proposed.id),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("already executed")
    ));
    // Before the network confirms it, the account shows what it holds once
    // the network does; the recipient shows nothing yet.
    assert_eq!(net.balance(main.id), change);
    assert_eq!(net.balance(savings.id), 0);

    net.mine(1, BURN);
    assert_eq!(net.balance(savings.id), BTC);
    assert_eq!(net.balance(main.id), change);

    // Its Bitcoin accounts keep the network: an upgrade cannot change it.
    let mainnet = WalletArg::Upgrade(Some(UpgradeArgs {
        bitcoin_network: Some(BitcoinNetwork::Mainnet),
    }));
    let refused = upgrade(&net.pic, net.wallet, &mainnet).unwrap_err();
    assert!(
        refused.contains("its Bitcoin accounts keep it"),
        "{refused}"
    );
    // The keys are the same after an upgrade: the savings account spends.
    upgrade(&net.pic, net.wallet, &WalletArg::Upgrade(None)).unwrap();
    assert_eq!(
        accounts(&net.pic, net.wallet, net.owner),
        [ic, main.clone(), savings.clone()]
    );
    let back = net
        .propose(bitcoin_transfer(savings.id, &main.address, BTC / 2, None))
        .unwrap();
    net.send(back.id);
    net.mine(1, BURN);
    assert_eq!(net.balance(main.id), change + BTC / 2);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn a_bitcoin_transfer_is_checked_and_spends_no_output_twice() {
    let net = wallet_on_regtest();
    let main = net.main_account();
    let mainnet = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
    let with_memo = Action::Transfer(TransferRequest {
        memo: Some(ByteBuf::from(vec![1])),
        ..match bitcoin_transfer(main.id, BURN, BTC, None) {
            Action::Transfer(request) => request,
            _ => unreachable!(),
        }
    });
    for action in [
        bitcoin_transfer(main.id, mainnet, BTC, None),
        bitcoin_transfer(main.id, "not an address", BTC, None),
        // P2WPKH outputs below 294 satoshis are dust.
        bitcoin_transfer(main.id, BURN, 293, None),
        with_memo,
    ] {
        assert!(
            matches!(
                net.propose(action.clone()),
                Err(WalletError::InvalidArgument(_))
            ),
            "{action:?}"
        );
    }

    // Two outputs of 50 BTC.
    net.mine(2, &main.address);
    net.mine(100, BURN);
    let first = net
        .propose(bitcoin_transfer(main.id, BURN, 30 * BTC, None))
        .unwrap();
    let second = net
        .propose(bitcoin_transfer(main.id, BURN, 30 * BTC, None))
        .unwrap();
    // Two executions of one transfer at once: one sends, one is refused.
    let execute = || {
        net.pic
            .submit_call(
                net.wallet,
                net.owner,
                "execute",
                encode_one(first.id).unwrap(),
            )
            .unwrap()
    };
    let (one, two) = (execute(), execute());
    let decode = |reply: Vec<u8>| decode_one::<PendingResult>(&reply).unwrap();
    let mut results = [
        decode(net.pic.await_call(one).unwrap()),
        decode(net.pic.await_call(two).unwrap()),
    ];
    results.sort_by_key(|result| result.is_err());
    let Ok(Pending {
        state: ActionState::Executed(Outcome::Sent(receipt)),
        ..
    }) = &results[0]
    else {
        panic!("{results:?}");
    };
    assert!(
        matches!(&results[1], Err(WalletError::InvalidArgument(reason)) if reason.contains("in flight")),
        "{results:?}"
    );
    net.wait_for_mempool(&receipt.tx_id);
    let first_tx = net.node.transaction(&receipt.tx_id);
    // The second transfer cannot spend the output the first one spends,
    // though neither is confirmed: both are in the mempool.
    let second_tx = net.send(second.id);
    let spent = |tx: &bitcoin::Transaction| -> Vec<bitcoin::OutPoint> {
        tx.input.iter().map(|input| input.previous_output).collect()
    };
    assert!(
        spent(&first_tx)
            .iter()
            .all(|outpoint| !spent(&second_tx).contains(outpoint))
    );
    assert!(net.node.mempool().contains(&receipt.tx_id));
    net.mine(1, BURN);
    let left = net.balance(main.id);
    assert!(left < 40 * BTC && left > 40 * BTC - 10_000, "{left}");

    let too_much = net
        .propose(bitcoin_transfer(main.id, BURN, 40 * BTC, None))
        .unwrap();
    let failed = net.execute(too_much.id).unwrap();
    assert!(
        matches!(&failed.state, ActionState::Failed(reason) if reason.contains("can spend")),
        "{failed:?}"
    );
    let too_low = net
        .propose(bitcoin_transfer(main.id, BURN, BTC, Some(10)))
        .unwrap();
    let failed = net.execute(too_low.id).unwrap();
    assert!(
        matches!(&failed.state, ActionState::Failed(reason) if reason.contains("below 1 satoshi per vbyte")),
        "{failed:?}"
    );
    // The failed transfers reserved nothing: both outputs are spent next.
    let fixed = net
        .propose(bitcoin_transfer(main.id, BURN, 39 * BTC, Some(5_000)))
        .unwrap();
    let tx = net.send(fixed.id);
    assert_eq!(tx.input.len(), 2);
    let paid: u64 = tx.output.iter().map(|output| output.value.to_sat()).sum();
    assert_eq!(left - paid, 5_000);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn the_daily_limit_counts_the_bitcoin_fee() {
    let net = wallet_on_regtest_after(|pic, wallet, owner| {
        let mut policy = policy(pic, wallet, owner);
        let rule = policy
            .rules
            .iter_mut()
            .find(|rule| rule.kind == ActionKind::Transfer)
            .unwrap();
        rule.timelock_ns = DAY_NS;
        rule.daily_limits = vec![DailyLimit {
            chain: REGTEST,
            amount: BTC.into(),
        }];
        let set_policy = propose(pic, wallet, owner, Action::SetPolicy(policy)).unwrap();
        pic.advance_time(Duration::from_nanos(7 * DAY_NS));
        act(pic, wallet, owner, "execute", set_policy.id).unwrap();
    });
    let main = net.main_account();
    net.mine(1, &main.address);
    net.mine(100, BURN);
    let at_once = |pending: &Pending| pending.executable_after_ns == pending.proposed_at_ns;

    // Released, but its fee takes it over the limit when it runs.
    let close = net
        .propose(bitcoin_transfer(main.id, BURN, BTC - 100, None))
        .unwrap();
    assert!(at_once(&close));
    let failed = net.execute(close.id).unwrap();
    assert!(
        matches!(&failed.state, ActionState::Failed(reason) if reason.contains("no longer fits")),
        "{failed:?}"
    );

    let half = net
        .propose(bitcoin_transfer(main.id, BURN, BTC / 2, Some(1_000)))
        .unwrap();
    assert!(at_once(&half));
    net.send(half.id);
    // A transfer spends only confirmed outputs, such as this change.
    net.mine(1, BURN);
    // Half a bitcoin and 1,000 satoshis are spent: this would pass the limit.
    let over = net
        .propose(bitcoin_transfer(main.id, BURN, BTC / 2, None))
        .unwrap();
    assert_eq!(over.executable_after_ns, over.proposed_at_ns + DAY_NS);
    let rest = net
        .propose(bitcoin_transfer(
            main.id,
            BURN,
            BTC / 2 - 2_000,
            Some(1_000),
        ))
        .unwrap();
    assert!(at_once(&rest));
    net.send(rest.id);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn a_send_that_may_have_reached_the_bitcoin_canister_sends_the_same_transaction_again() {
    let net = wallet_on_regtest();
    let main = net.main_account();
    net.mine(1, &main.address);
    net.mine(100, BURN);
    let transfer = net
        .propose(bitcoin_transfer(main.id, BURN, BTC, None))
        .unwrap();

    // The Bitcoin canister refuses a transaction that carries fewer cycles
    // than it asks, after its call began: the wallet cannot tell whether it
    // passed the transaction on.
    let fees = net.bitcoin_fees();
    net.set_bitcoin_fees(Fees {
        send_transaction_base: 1_000 * 1_000_000_000_000,
        ..fees.clone()
    });
    let unknown = net.execute(transfer.id);
    assert!(
        matches!(&unknown, Err(WalletError::ChainError(reason)) if reason.contains("send the same one")),
        "{unknown:?}"
    );
    let open = net.pending(transfer.id);
    assert_eq!(open.state, ActionState::Ready);
    assert_eq!(open.expires_at_ns, u64::MAX);
    assert!(matches!(
        act(&net.pic, net.wallet, net.owner, "reject", transfer.id),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("may have reached the Bitcoin canister")
    ));
    // Its only output stays reserved for it.
    let other = net
        .propose(bitcoin_transfer(main.id, BURN, BTC, None))
        .unwrap();
    let failed = net.execute(other.id).unwrap();
    assert!(
        matches!(&failed.state, ActionState::Failed(reason) if reason.contains("can spend 0")),
        "{failed:?}"
    );

    net.set_bitcoin_fees(fees);
    let tx = net.send(transfer.id);
    let hash = net.node.call(&["getblockhash", "1"]);
    let block: bitcoin::Block =
        bitcoin::consensus::encode::deserialize_hex(&net.node.call(&["getblock", &hash, "0"]))
            .unwrap();
    assert_eq!(tx.input.len(), 1);
    assert_eq!(
        tx.input[0].previous_output.txid,
        block.txdata[0].compute_txid()
    );
    assert_eq!(outputs(&tx)[0], (BURN.to_string(), BTC));
    assert_eq!(net.node.mempool().matches('"').count(), 2);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn the_wallet_sends_a_transfer_again_an_hour_after_a_send_it_could_not_tell() {
    let net = wallet_on_regtest();
    let main = net.main_account();
    net.mine(1, &main.address);
    net.mine(100, BURN);
    let transfer = net
        .propose(bitcoin_transfer(main.id, BURN, BTC, None))
        .unwrap();
    // As above, the Bitcoin canister refuses the send after its call began.
    let fees = net.bitcoin_fees();
    net.set_bitcoin_fees(Fees {
        send_transaction_base: 1_000 * 1_000_000_000_000,
        ..fees.clone()
    });
    assert!(matches!(
        net.execute(transfer.id),
        Err(WalletError::ChainError(reason)) if reason.contains("send the same one")
    ));
    net.set_bitcoin_fees(fees);

    // The wallet's timer looks every ten minutes, and sends a transaction
    // again an hour after its last send.
    net.pic.advance_time(Duration::from_secs(50 * 60));
    net.run_timer();
    assert_eq!(net.pending(transfer.id).state, ActionState::Ready);
    assert_eq!(net.mempool(), Vec::<String>::new());

    net.pic.advance_time(Duration::from_secs(20 * 60));
    net.tick_until("the wallet did not send the transfer again", || {
        net.mempool().len() == 1
    });
    let tx = net.node.transaction(&net.mempool()[0]);
    assert_eq!(outputs(&tx)[0], (BURN.to_string(), BTC));
    // The send ended the transfer, as the wallet's own execution.
    assert_eq!(pending(&net.pic, net.wallet, net.owner), []);
    assert_eq!(
        audit(&net.pic, net.wallet, net.owner).last(),
        Some(&(None, AuditEvent::Executed(transfer.id)))
    );
    net.mine(1, BURN);
    assert_eq!(net.balance(main.id), outputs(&tx)[1].1);

    // A confirmed transaction is never sent again.
    let sends = net.sends();
    net.pic.advance_time(Duration::from_secs(3 * 60 * 60));
    net.run_timer();
    assert_eq!(net.sends(), sends);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn the_wallet_sends_a_transaction_the_network_dropped_again_and_again() {
    let mut net = wallet_on_regtest();
    let main = net.main_account();
    net.mine(1, &main.address);
    net.mine(100, BURN);
    let transfer = net
        .propose(bitcoin_transfer(main.id, BURN, BTC, None))
        .unwrap();
    let txid = net.send_txid(transfer.id);
    let change = outputs(&net.node.transaction(&txid))[1].1;
    assert_eq!(net.sends(), 1);

    // The node forgets the transaction; the account still shows it spent.
    net.node.restart(&["-persistmempool=0"]);
    assert_eq!(net.mempool(), Vec::<String>::new());
    assert_eq!(net.balance(main.id), change);

    // While the account's output stays unspent, the wallet's timer hands
    // the transaction to the Bitcoin canister again an hour after the send,
    // then two hours after that. (PocketIC's adapter passes a transaction on
    // once in ten minutes of real time, so the node does not see these.)
    for (wait_minutes, sends) in [(50, 1), (20, 2), (110, 2), (20, 3)] {
        net.pic.advance_time(Duration::from_secs(wait_minutes * 60));
        net.run_timer();
        assert_eq!(net.sends(), sends, "after {wait_minutes} more minutes");
    }
    assert_eq!(net.balance(main.id), change);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn a_transfer_spends_outputs_100_blocks_old_before_a_larger_young_mining_reward() {
    let net = wallet_on_regtest();
    let main = net.main_account();
    net.mine(1, &main.address);
    net.mine(100, BURN);
    let first = net
        .propose(bitcoin_transfer(main.id, BURN, 40 * BTC, None))
        .unwrap();
    let first = net.send(first.id);
    assert_eq!(outputs(&first)[1].0, main.address);
    let change = bitcoin::OutPoint::new(first.compute_txid(), 1);
    // The change confirms in block 102.
    net.mine(99, BURN);
    // Block 201, where the change has exactly 100 confirmations, pays the
    // account 25 BTC: more than the change, and unspendable before block 301.
    net.mine(1, &main.address);
    assert_eq!(net.balance(main.id), outputs(&first)[1].1 + 25 * BTC);

    let second = net
        .propose(bitcoin_transfer(main.id, BURN, 5 * BTC, None))
        .unwrap();
    let second = net.send(second.id);
    assert_eq!(second.input.len(), 1);
    assert_eq!(second.input[0].previous_output, change);
}

#[test]
#[ignore = "needs bitcoind: set BITCOIND and BITCOIN_CLI"]
fn a_policy_that_changes_while_a_transfer_signs_applies_before_it_sends() {
    const SECOND_NS: u64 = 1_000_000_000;
    // Every rule waits at most a second, so a new policy can apply while a
    // transfer signs; transfers within `amount` a day wait for nothing.
    let limited = |mut policy: Policy, amount: u64| {
        for rule in &mut policy.rules {
            rule.timelock_ns = rule.timelock_ns.min(SECOND_NS);
            if rule.kind == ActionKind::Transfer {
                rule.timelock_ns = SECOND_NS;
                rule.daily_limits = vec![DailyLimit {
                    chain: REGTEST,
                    amount: amount.into(),
                }];
            }
        }
        policy
    };
    let net = wallet_on_regtest_after(|pic, wallet, owner| {
        let policy = limited(policy(pic, wallet, owner), 100 * BTC);
        let set_policy = propose(pic, wallet, owner, Action::SetPolicy(policy)).unwrap();
        pic.advance_time(Duration::from_nanos(7 * DAY_NS));
        act(pic, wallet, owner, "execute", set_policy.id).unwrap();
    });
    let main = net.main_account();
    // Three outputs of 50 BTC.
    net.mine(3, &main.address);
    net.mine(100, BURN);
    let lower = net
        .propose(Action::SetPolicy(limited(
            policy(&net.pic, net.wallet, net.owner),
            BTC,
        )))
        .unwrap();
    net.pic.advance_time(Duration::from_secs(2));

    // A transfer whose send may have reached the Bitcoin canister, as in
    // the test above.
    let maybe_sent = net
        .propose(bitcoin_transfer(main.id, BURN, 30 * BTC, Some(10_000)))
        .unwrap();
    assert_eq!(maybe_sent.executable_after_ns, maybe_sent.proposed_at_ns);
    let fees = net.bitcoin_fees();
    net.set_bitcoin_fees(Fees {
        send_transaction_base: 1_000 * 1_000_000_000_000,
        ..fees.clone()
    });
    assert!(matches!(
        net.execute(maybe_sent.id),
        Err(WalletError::ChainError(reason)) if reason.contains("send the same one")
    ));
    net.set_bitcoin_fees(fees);

    // Two outputs left, so this transfer asks for two signatures.
    let transfer = net
        .propose(bitcoin_transfer(main.id, BURN, 60 * BTC, Some(10_000)))
        .unwrap();
    assert_eq!(transfer.executable_after_ns, transfer.proposed_at_ns);

    let call = net
        .pic
        .submit_call(
            net.wallet,
            net.owner,
            "execute",
            encode_one(transfer.id).unwrap(),
        )
        .unwrap();
    // The wallet pays for reading its outputs, then for the first signature,
    // each at least a billion cycles: by then the transaction is built and
    // passed the daily limit.
    let (start, mut last, mut paid) = (Instant::now(), net.pic.cycle_balance(net.wallet), 0);
    while paid < 2 {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the transfer never asked for a signature"
        );
        net.pic.tick();
        let balance = net.pic.cycle_balance(net.wallet);
        if last > balance + 1_000_000_000 {
            paid += 1;
        }
        last = balance;
    }
    let lowered = net.execute(lower.id).unwrap();
    assert_eq!(lowered.state, ActionState::Executed(Outcome::Done));

    let executed = decode_one::<PendingResult>(&net.pic.await_call(call).unwrap())
        .unwrap()
        .unwrap();
    assert!(
        matches!(&executed.state, ActionState::Failed(reason) if reason.contains("no longer fits")),
        "{executed:?}"
    );
    // The new limit does not hold back a transaction that may have been
    // sent: it goes as it was signed.
    let resent = net.send(maybe_sent.id);
    assert_eq!(outputs(&resent)[0], (BURN.to_string(), 30 * BTC));
    // The failed transfer released its outputs: after its timelock, a
    // transfer spends one.
    let small = net
        .propose(bitcoin_transfer(main.id, BURN, BTC / 2, Some(10_000)))
        .unwrap();
    net.pic.advance_time(Duration::from_secs(2));
    net.send(small.id);
}
