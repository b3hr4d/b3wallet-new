//! The real wallet Wasm in PocketIC. Build it first:
//! `cargo build -p wallet --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.

use candid::types::{FuncMode, TypeInner};
use candid::{CandidType, Deserialize, Principal, decode_one, encode_args, encode_one};
use candid_parser::utils::CandidSource;
use ed25519_dalek::{Signer, SigningKey};
use ic_http_certification::{HttpRequest, StatusCode};
use ic_response_verification::types::VerifiedResponse;
use ic_response_verification::verify_request_response_pair;
use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
use pocket_ic::{ErrorCode, PocketIc, PocketIcBuilder, RejectCode, RejectResponse};
use serde_bytes::ByteBuf;
use std::path::Path;
use std::time::Duration;
use wallet::{
    Account, AccountHolder, Action, ActionId, ActionKind, ActionState, AddAccountArgs, AuditEntry,
    AuditEvent, AuditRequest, Balance, Chain, ClaimArgs, DailyLimit, InitArgs, Member,
    NOT_A_MEMBER, NOT_AN_APPROVER, NOT_AN_OWNER, NOT_CLAIMED, Outcome, Pending, Policy, Role,
    Status, TransferRequest, UpgradeState, WalletArg, WalletError,
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
        assert_eq!(
            refused,
            [
                "accounts",
                "add_account",
                "approve",
                "audit_log",
                "balance",
                "execute",
                "members",
                "pending",
                "policy",
                "propose",
                "reject",
                "request_join",
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
                "audit_log",
                "balance",
                "execute",
                "members",
                "pending",
                "policy",
                "propose",
                "reject"
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
    Action::AddMember {
        id,
        role,
        label: "device".to_string(),
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

    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
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
    let (approver, viewer) = (user(8), user(9));
    run_at_once(&pic, id, owner, add_member_action(approver, Role::Approver));
    run_at_once(&pic, id, owner, add_member_action(viewer, Role::Viewer));

    let refusals = [
        (approver, "propose", NOT_AN_OWNER),
        (approver, "reject", NOT_AN_OWNER),
        (approver, "add_account", NOT_AN_OWNER),
        (viewer, "propose", NOT_AN_OWNER),
        (viewer, "add_account", NOT_AN_OWNER),
        (viewer, "reject", NOT_AN_OWNER),
        (viewer, "approve", NOT_AN_APPROVER),
        (viewer, "execute", NOT_AN_APPROVER),
    ];
    for (caller, method, refusal) in refusals {
        let reject = call_without_arguments(&pic, id, caller, method, false).unwrap_err();
        assert_eq!(reject.reject_code, RejectCode::CanisterReject, "{method}");
        assert_eq!(reject.reject_message, refusal, "{method}");
    }
    // Viewers read.
    assert_eq!(pending(&pic, id, viewer), []);
    assert_eq!(accounts(&pic, id, viewer), []);
    assert_eq!(policy(&pic, id, viewer), policy(&pic, id, owner));
    assert_eq!(members(&pic, id, viewer).unwrap().len(), 3);
}

/// A wallet that the previous release installed, that its owner claimed and
/// that holds an open action, upgrades to this one, keeps its owner and the
/// action, and runs this release's code. CI
/// builds the previous release from the pull request's base commit (on
/// `main`, from the commit before) and names it in `PREVIOUS_WALLET_WASM`.
/// A change that leaves the Wasm as it was upgrades a wallet to itself.
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

    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
    assert!(status(&pic, id).claimed);
    assert_eq!(whoami(&pic, id, owner).map(|m| m.role), Some(Role::Owner));
    assert_eq!(members(&pic, id, owner).unwrap().len(), 1);
    assert_eq!(pending(&pic, id, owner), std::slice::from_ref(&open));
    let executed = act(&pic, id, owner, "execute", open.id).unwrap();
    assert_eq!(executed.state, ActionState::Executed(Outcome::Done));
    assert_eq!(members(&pic, id, owner).unwrap().len(), 2);
    add_account(&pic, id, owner, Chain::Icp, "main").unwrap();
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
    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
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
const NOT_FOUND_HTML: &[u8] = include_bytes!("../../../canisters/wallet/pages/404.html");

/// The Content-Security-Policy from decision 0001, spelled out here so that
/// any change to it fails this test.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
    img-src 'self' data:; font-src 'self'; \
    connect-src 'self' https://icp-api.io https://icp0.io; frame-src 'none'; \
    frame-ancestors 'none'; object-src 'none'; base-uri 'none'; \
    form-action 'none'; upgrade-insecure-requests";

/// The headers every page carries, certified.
const PAGE_HEADERS: &[(&str, &str)] = &[
    ("content-type", "text/html; charset=utf-8"),
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
    let pages: &[(&str, u16, &[u8])] = &[
        ("/", 200, INDEX_HTML),
        ("/index.html", 200, INDEX_HTML),
        ("/?from=link", 200, INDEX_HTML),
        ("/404.html", 200, NOT_FOUND_HTML),
        ("/claim", 404, NOT_FOUND_HTML),
        ("/assets/app.js", 404, NOT_FOUND_HTML),
    ];
    for &(url, status, body) in pages {
        let (request, response) = http_get(pic, id, url);
        assert_eq!(response.status_code().as_u16(), status, "{url}");
        assert_eq!(response.body(), body, "{url}");
        let certified = verify(pic, id, request, response).unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(certified.status_code, Some(status), "{url}");
        assert_eq!(certified.body, body, "{url}");
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

    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
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
    let (_, not_found) = http_get(&pic, id, "/claim");
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
    chain: Chain,
    label: &str,
) -> Result<Account, WalletError> {
    let args = AddAccountArgs {
        chain,
        label: label.to_string(),
    };
    update(pic, id, caller, "add_account", &args)
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
    update::<Result<Balance, WalletError>>(pic, id, caller, "balance", &account).map(|balance| {
        assert_eq!(balance.account, account);
        u64::try_from(balance.amount.0).unwrap()
    })
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

#[test]
fn accounts_are_the_wallets_principal_and_its_subaccounts() {
    let pic = PocketIc::new();
    let (id, owner) = claimed_wallet(&pic);
    let viewer = user(9);
    run_at_once(&pic, id, owner, add_member_action(viewer, Role::Viewer));
    let token = Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap();

    let main = add_account(&pic, id, owner, Chain::Icp, "main").unwrap();
    assert_eq!(
        main,
        Account {
            id: 1,
            chain: Chain::Icp,
            label: "main".to_string(),
            index: 0,
            holder: AccountHolder::Wallet,
            address: id.to_text(),
        }
    );
    let savings = add_account(&pic, id, owner, Chain::Icp, "savings").unwrap();
    assert_eq!((savings.id, savings.index), (2, 1));
    assert_eq!(savings.address, wallet_account(id, 1).to_string());
    let tokens = add_account(&pic, id, owner, Chain::Icrc(token), "tokens").unwrap();
    assert_eq!((tokens.id, tokens.index), (3, 0));
    assert_eq!(tokens.address, id.to_text());

    let refused = [
        (Chain::Bitcoin(wallet::BitcoinNetwork::Regtest), "btc"),
        (Chain::Evm(1), "eth"),
        (Chain::Icrc(icp_ledger()), "icp again"),
        (Chain::Icrc(user(3)), "not a ledger"),
        (Chain::Icp, ""),
    ];
    for (chain, label) in refused {
        assert!(
            matches!(
                add_account(&pic, id, owner, chain.clone(), label),
                Err(WalletError::InvalidArgument(_))
            ),
            "{chain:?}"
        );
    }
    let reject = pic
        .update_call(
            id,
            viewer,
            "add_account",
            encode_one(AddAccountArgs {
                chain: Chain::Icp,
                label: "mine".to_string(),
            })
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(reject.reject_message, NOT_AN_OWNER);

    let all = vec![main, savings, tokens];
    assert_eq!(accounts(&pic, id, viewer), all);
    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
    assert_eq!(accounts(&pic, id, owner), all);
    assert_eq!(
        audit_since(&pic, id, owner, 3),
        [
            (Some(owner), AuditEvent::AccountAdded(1)),
            (Some(owner), AuditEvent::AccountAdded(2)),
            (Some(owner), AuditEvent::AccountAdded(3)),
        ]
    );
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
    add_account(&pic, wallet, owner, Chain::Icp, "main").unwrap();
    add_account(&pic, wallet, owner, Chain::Icp, "savings").unwrap();
    add_account(&pic, wallet, owner, Chain::Icrc(token), "tokens").unwrap();

    assert_eq!(balance(&pic, wallet, viewer, 1), Ok(5 * E8S));
    assert_eq!(balance(&pic, wallet, viewer, 2), Ok(0));
    assert_eq!(balance(&pic, wallet, owner, 3), Ok(1_000));
    assert_eq!(
        balance(&pic, wallet, owner, 4),
        Err(WalletError::NotFound("account 4".to_string()))
    );

    pic.stop_canister(token, None).unwrap();
    assert!(matches!(
        balance(&pic, wallet, owner, 3),
        Err(WalletError::ChainError(reason)) if reason.contains("icrc1_balance_of")
    ));
    pic.start_canister(token, None).unwrap();
    assert_eq!(balance(&pic, wallet, owner, 3), Ok(1_000));
}

#[test]
fn a_transfer_sends_from_its_accounts_subaccount_once() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    add_account(&pic, wallet, owner, Chain::Icp, "main").unwrap();
    let savings = add_account(&pic, wallet, owner, Chain::Icp, "savings").unwrap();
    let friend = LedgerAccount::from(user(20));
    let before = audit(&pic, wallet, owner).len();

    // From account 0 to the wallet's own subaccount 1, with a memo.
    let to_savings = Action::Transfer(TransferRequest {
        from: 1,
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
    add_account(&pic, id, owner, Chain::Icp, "main").unwrap();
    let friend = LedgerAccount::from(user(20)).to_string();
    let request = |from, to: &str, amount: u64, memo: usize| {
        Action::Transfer(TransferRequest {
            from,
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
    add_account(&pic, wallet, owner, Chain::Icrc(token), "tokens").unwrap();
    let friend = LedgerAccount::from(user(20));
    let proposed = propose(&pic, wallet, owner, transfer_action(1, friend, 100)).unwrap();

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
fn a_second_execute_while_the_first_is_in_flight_is_refused() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    add_account(&pic, wallet, owner, Chain::Icp, "main").unwrap();
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

#[test]
fn a_transfer_the_ledger_refuses_fails() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    add_account(&pic, wallet, owner, Chain::Icp, "main").unwrap();
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
    add_account(&pic, wallet, owner, Chain::Icp, "main").unwrap();
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

#[test]
fn a_transfer_under_the_daily_limit_is_checked_again_when_it_runs() {
    let Ledgers {
        pic, wallet, owner, ..
    } = wallet_with_ledgers();
    add_account(&pic, wallet, owner, Chain::Icp, "main").unwrap();
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
