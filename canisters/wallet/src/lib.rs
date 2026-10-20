//! Firmware of a self-owned chain-key wallet canister.
//!
//! Steps 1 to 5 of `docs/plan-v0.md`: the install arguments, `status`, state
//! in stable memory, the claim, members, and the policy with its pending
//! actions. The interface test lists the methods of `candid/wallet.did` not
//! implemented yet.

mod access;
mod actions;
mod claim;
mod ed25519;
mod governance;
mod policy;
mod storage;
mod types;

pub use access::{NOT_A_MEMBER, NOT_AN_APPROVER, NOT_AN_OWNER, NOT_CLAIMED};
pub use claim::message as claim_message;
pub use types::*;

use actions::{
    APPROVAL_WINDOW_NS, JOIN_WINDOW_NS, MAX_OPEN_ACTIONS, MAX_OPEN_JOIN_REQUESTS, StoredPending,
};
use candid::{Nat, Principal};
use storage::StoredMember;

#[ic_cdk::init]
fn init(arg: WalletArg) {
    let WalletArg::Init(args) = arg else {
        ic_cdk::trap("install needs WalletArg::Init");
    };
    if let Err(reason) = args.validate(ic_cdk::api::time()) {
        ic_cdk::trap(format!("invalid InitArgs: {reason}"));
    }
    storage::set_config(args.into());
    storage::set_policy(policy::default_policy());
}

// A trap here makes the IC keep the running version: an upgrade either reads
// all of the state or does not happen.
#[ic_cdk::post_upgrade]
fn post_upgrade(arg: WalletArg) {
    if !matches!(arg, WalletArg::Upgrade) {
        ic_cdk::trap("an upgrade needs WalletArg::Upgrade");
    }
    storage::migrate();
    if let Err(reason) = storage::check() {
        ic_cdk::trap(format!("the upgrade cannot read the state: {reason}"));
    }
}

/// Makes the caller the first owner. See `claim::check` for what it refuses.
#[ic_cdk::update]
fn claim(args: ClaimArgs) -> Result<Member, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let now = ic_cdk::api::time();
    let Some(config) = storage::config() else {
        ic_cdk::trap("the wallet has no configuration");
    };
    claim::check(
        config.claim.as_ref(),
        storage::is_claimed(),
        ic_cdk::api::canister_self(),
        caller,
        now,
        &args.signature,
    )?;
    let member = StoredMember {
        role: Role::Owner,
        label: "first owner".to_string(),
        added_at_ns: now,
        added_by: None,
    };
    storage::complete_claim(caller, member.clone());
    Ok(member.into_member(caller))
}

/// The caller's membership; `None` for anyone who is not a member.
#[ic_cdk::query(guard = "access::claimed")]
fn whoami() -> Option<Member> {
    let caller = ic_cdk::api::msg_caller();
    storage::member(caller).map(|member| member.into_member(caller))
}

#[ic_cdk::query(guard = "access::member")]
fn members() -> Result<Vec<Member>, WalletError> {
    Ok(storage::members())
}

#[ic_cdk::query(guard = "access::member")]
fn policy() -> Result<Policy, WalletError> {
    Ok(storage::policy())
}

/// The open actions, oldest first.
#[ic_cdk::query(guard = "access::member")]
fn pending() -> Result<Vec<Pending>, WalletError> {
    let now = ic_cdk::api::time();
    Ok(storage::open_actions(now)
        .into_iter()
        .map(|(id, pending)| pending.view(id, now))
        .collect())
}

/// Proposes an action. The proposer's approval counts toward its quorum, so
/// with a quorum of one it is ready at once.
#[ic_cdk::update(guard = "access::owner")]
fn propose(action: Action) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let governance = storage::governance();
    governance.check(&action, caller, now)?;
    storage::close_expired(now);
    let open = storage::open_actions(now);
    if open.iter().filter(|(_, p)| !p.is_join_request()).count() >= MAX_OPEN_ACTIONS {
        return Err(WalletError::Forbidden(format!(
            "at most {MAX_OPEN_ACTIONS} actions can be open at once"
        )));
    }
    let terms = policy::terms(&governance.policy, action.kind(), None);
    let pending = StoredPending::new(action, caller, vec![caller], terms, APPROVAL_WINDOW_NS, now);
    let id = storage::add_action(pending.clone());
    Ok(pending.view(id, now))
}

/// Asks to join as a viewer. It is an `AddMember` action that the members
/// approve like any other. Asking again while a request is open returns it.
#[ic_cdk::update(guard = "access::claimed")]
fn request_join(label: String) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    if caller == Principal::anonymous() {
        return Err(WalletError::Forbidden(
            "the anonymous principal cannot join".to_string(),
        ));
    }
    let governance = storage::governance();
    if governance.members.contains_key(&caller) {
        return Err(WalletError::Forbidden(
            "the caller is already a member".to_string(),
        ));
    }
    storage::close_expired(now);
    let open = storage::open_actions(now);
    let requests: Vec<&(ActionId, StoredPending)> =
        open.iter().filter(|(_, p)| p.is_join_request()).collect();
    if let Some((id, pending)) = requests.iter().find(|(_, p)| p.proposed_by == caller) {
        return Ok(pending.view(*id, now));
    }
    if requests.len() >= MAX_OPEN_JOIN_REQUESTS {
        return Err(WalletError::Forbidden(format!(
            "at most {MAX_OPEN_JOIN_REQUESTS} join requests can be open at once"
        )));
    }
    let action = Action::AddMember {
        id: caller,
        role: Role::Viewer,
        label,
    };
    governance.check(&action, caller, now)?;
    let terms = policy::terms(&governance.policy, ActionKind::AddMember, None);
    let pending = StoredPending::new(action, caller, vec![], terms, JOIN_WINDOW_NS, now);
    let id = storage::add_action(pending.clone());
    Ok(pending.view(id, now))
}

#[ic_cdk::update(guard = "access::approver")]
fn approve(id: ActionId) -> Result<Pending, WalletError> {
    change_action(id, |pending, caller, now| pending.approve(id, caller, now))
}

#[ic_cdk::update(guard = "access::owner")]
fn reject(id: ActionId) -> Result<Pending, WalletError> {
    change_action(id, |pending, caller, now| pending.reject(id, caller, now))
}

/// Runs an action that has its quorum once its timelock has passed. An
/// action that no longer fits the members or the policy ends `Failed`.
#[ic_cdk::update(guard = "access::approver")]
fn execute(id: ActionId) -> Result<Pending, WalletError> {
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    if let Err(error) = pending.check_executable(id, now) {
        storage::set_action(id, pending);
        return Err(error);
    }
    let mut governance = storage::governance();
    // The first approval is the proposer's, or for a join request the first
    // member who approved it.
    let added_by = pending
        .approvals
        .first()
        .copied()
        .unwrap_or(pending.proposed_by);
    let result = governance.apply(&pending.action, added_by, now);
    if result.is_ok() {
        storage::save_governance(&governance);
        // An approval counts only while its member can approve.
        for (open_id, mut open) in storage::open_actions(now) {
            let before = open.approvals.len();
            for approver in open.approvals.clone() {
                if !governance.can_approve(&approver) {
                    open.drop_approval(approver);
                }
            }
            if open.approvals.len() != before {
                storage::set_action(open_id, open);
            }
        }
    }
    pending.finish(
        result
            .map(|()| Outcome::Done)
            .map_err(|error| reason(&error)),
    );
    storage::set_action(id, pending.clone());
    Ok(pending.view(id, now))
}

fn stored_action(id: ActionId) -> Result<StoredPending, WalletError> {
    storage::action(id).ok_or_else(|| WalletError::NotFound(format!("action {id}")))
}

/// Applies `change` to a stored action and stores the result, also when it
/// refuses: a refusal may have written down an expiry.
fn change_action(
    id: ActionId,
    change: impl FnOnce(&mut StoredPending, Principal, u64) -> Result<(), WalletError>,
) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let mut pending = stored_action(id)?;
    let result = change(&mut pending, caller, now);
    storage::set_action(id, pending.clone());
    result.map(|()| pending.view(id, now))
}

fn reason(error: &WalletError) -> String {
    match error {
        WalletError::Forbidden(reason)
        | WalletError::NotFound(reason)
        | WalletError::InvalidArgument(reason)
        | WalletError::PolicyViolation(reason) => reason.clone(),
        other => format!("{other:?}"),
    }
}

#[ic_cdk::query]
fn status() -> Status {
    let Some(config) = storage::config() else {
        ic_cdk::trap("the wallet has no configuration");
    };
    Status {
        version: env!("CARGO_PKG_VERSION").to_string(),
        claimed: storage::is_claimed(),
        claim_expires_at_ns: config.claim.map(|claim| claim.expires_at_ns),
        cycles: Nat::from(ic_cdk::api::canister_cycle_balance()),
        upgrade: UpgradeState::Idle,
        release_channel: config.release_channel,
    }
}

ic_cdk::export_candid!();

#[cfg(test)]
mod interface {
    use candid::TypeEnv;
    use candid::types::subtype::equal;
    use candid::types::{Type, TypeInner};
    use candid_parser::utils::CandidSource;
    use std::collections::HashSet;
    use std::path::Path;

    const TARGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/wallet.did");

    fn class(env: &TypeEnv, actor: &Type) -> (Vec<Type>, Vec<(String, Type)>) {
        let actor = env.trace_type(actor).expect("actor type");
        let TypeInner::Class(args, service) = actor.as_ref() else {
            panic!("the interface has no init arguments: {actor}");
        };
        let methods = env.as_service(service).expect("service type").to_vec();
        (args.clone(), methods)
    }

    /// Every method the canister exports must exist in `candid/wallet.did`
    /// with an equal type, mode included, and the init arguments must match.
    /// Methods not implemented yet are printed, not failed.
    #[test]
    fn implemented_methods_match_the_target_interface() {
        let exported = super::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let (target_env, target_actor) = CandidSource::File(Path::new(TARGET))
            .load()
            .expect("candid/wallet.did parses");
        let target_actor = env.merge_type(target_env, target_actor.expect("target service"));
        let (exported_args, exported_methods) =
            class(&env, &exported_actor.expect("exported service"));
        let (target_args, target_methods) = class(&env, &target_actor);

        let mut gamma = HashSet::new();
        assert_eq!(
            exported_args.len(),
            target_args.len(),
            "init argument count"
        );
        for (exported, target) in exported_args.iter().zip(&target_args) {
            equal(&mut gamma, &env, exported, target)
                .unwrap_or_else(|e| panic!("init argument {exported} differs from {target}: {e}"));
        }

        let mut mismatches = Vec::new();
        for (name, exported) in &exported_methods {
            match target_methods
                .iter()
                .find(|(target_name, _)| target_name == name)
            {
                None => mismatches.push(format!("{name}: not in candid/wallet.did")),
                Some((_, target)) => {
                    if let Err(e) = equal(&mut gamma, &env, exported, target) {
                        mismatches.push(format!("{name}: {exported} differs from {target}: {e}"));
                    }
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "interface mismatches:\n{}",
            mismatches.join("\n")
        );

        let missing: Vec<&str> = target_methods
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| {
                !exported_methods
                    .iter()
                    .any(|(exported, _)| exported == name)
            })
            .collect();
        eprintln!(
            "implemented {} of {} methods; not yet: {}",
            exported_methods.len(),
            target_methods.len(),
            missing.join(", ")
        );
    }
}
