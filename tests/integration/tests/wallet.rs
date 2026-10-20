//! The real wallet Wasm in PocketIC. Build it first:
//! `cargo build -p wallet --target wasm32-unknown-unknown --release`.
//! PocketIC uses the server binary named by `POCKET_IC_BIN`.

use candid::types::{FuncMode, TypeInner};
use candid::{CandidType, Deserialize, Principal, decode_one, encode_args, encode_one};
use candid_parser::utils::CandidSource;
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::{ErrorCode, PocketIc, RejectCode, RejectResponse};
use serde_bytes::ByteBuf;
use std::path::Path;
use std::time::Duration;
use wallet::{
    Action, ActionId, ActionState, ClaimArgs, InitArgs, Member, NOT_A_MEMBER, NOT_AN_APPROVER,
    NOT_AN_OWNER, NOT_CLAIMED, Outcome, Pending, Policy, Role, Status, TransferRequest,
    UpgradeState, WalletArg, WalletError,
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
                "approve",
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
                "approve", "execute", "members", "pending", "policy", "propose", "reject"
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
    assert!(matches!(
        propose(&pic, id, owner, transfer),
        Err(WalletError::InvalidArgument(reason)) if reason.contains("cannot execute Transfer")
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
        (viewer, "propose", NOT_AN_OWNER),
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
    assert_eq!(policy(&pic, id, viewer), policy(&pic, id, owner));
    assert_eq!(members(&pic, id, viewer).unwrap().len(), 3);
}

/// A wallet that the previous release installed and that its owner claimed
/// upgrades to this one, keeps its owner, and runs this release's code. CI
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

    upgrade(&pic, id, &WalletArg::Upgrade).unwrap();
    assert!(status(&pic, id).claimed);
    assert_eq!(whoami(&pic, id, owner).map(|m| m.role), Some(Role::Owner));
    assert_eq!(members(&pic, id, owner).unwrap().len(), 1);
    run_at_once(&pic, id, owner, add_member_action(user(8), Role::Viewer));
}
