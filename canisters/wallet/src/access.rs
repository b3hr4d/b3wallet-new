//! Who may call what. A method that not everyone may call names one of these
//! guards. ic-cdk runs the guard before it decodes the arguments, and a refused
//! call is rejected with the guard's message, so a caller the wallet refuses
//! never gets it to decode anything.
//!
//! Anyone may call `claim`, `status` and `http_request`; once the wallet is
//! claimed, also `whoami` and `request_join`. Every other method is for
//! members only, except `ext_request`, which will have its own guard for
//! registered extensions. Of the members, only owners propose and reject,
//! owners and approvers approve and execute, and viewers only read.

use crate::{Role, policy, storage};
use candid::Principal;

pub const NOT_CLAIMED: &str = "the wallet is not claimed yet";
pub const NOT_A_MEMBER: &str = "the caller is not a member of this wallet";
pub const NOT_AN_OWNER: &str = "only an owner can do this";
pub const NOT_AN_APPROVER: &str = "only an owner or an approver can do this";

/// Guard: the wallet has its first owner.
pub fn claimed() -> Result<(), String> {
    if storage::is_claimed() {
        Ok(())
    } else {
        Err(NOT_CLAIMED.to_string())
    }
}

/// Guard: the caller is a member, of any role.
pub fn member() -> Result<(), String> {
    require_member(ic_cdk::api::msg_caller())
}

/// Guard: the caller is an owner.
pub fn owner() -> Result<(), String> {
    require_owner(ic_cdk::api::msg_caller())
}

/// Guard: the caller is an owner or an approver.
pub fn approver() -> Result<(), String> {
    require_approver(ic_cdk::api::msg_caller())
}

fn require_member(caller: Principal) -> Result<(), String> {
    require_role(caller, |_| true, NOT_A_MEMBER)
}

fn require_owner(caller: Principal) -> Result<(), String> {
    require_role(caller, |role| role == Role::Owner, NOT_AN_OWNER)
}

fn require_approver(caller: Principal) -> Result<(), String> {
    require_role(caller, policy::approves, NOT_AN_APPROVER)
}

fn require_role(caller: Principal, allowed: fn(Role) -> bool, refusal: &str) -> Result<(), String> {
    claimed()?;
    match storage::member(caller) {
        None => Err(NOT_A_MEMBER.to_string()),
        Some(member) if allowed(member.role) => Ok(()),
        Some(_) => Err(refusal.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoredMember;

    fn user(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn add(id: Principal, role: Role) {
        storage::insert_member(
            id,
            StoredMember {
                role,
                label: "test".to_string(),
                added_at_ns: 0,
                added_by: None,
            },
        );
    }

    #[test]
    fn before_the_claim_everyone_is_refused() {
        assert_eq!(claimed(), Err(NOT_CLAIMED.to_string()));
        assert_eq!(require_member(user(1)), Err(NOT_CLAIMED.to_string()));
        assert_eq!(
            require_member(Principal::anonymous()),
            Err(NOT_CLAIMED.to_string())
        );
    }

    #[test]
    fn after_the_claim_only_members_pass() {
        add(user(1), Role::Viewer);
        assert_eq!(claimed(), Ok(()));
        assert_eq!(require_member(user(1)), Ok(()));
        assert_eq!(require_member(user(2)), Err(NOT_A_MEMBER.to_string()));
        assert_eq!(
            require_member(Principal::anonymous()),
            Err(NOT_A_MEMBER.to_string())
        );
    }

    #[test]
    fn each_role_passes_only_its_guards() {
        add(user(1), Role::Owner);
        add(user(2), Role::Approver);
        add(user(3), Role::Viewer);
        assert_eq!(require_owner(user(1)), Ok(()));
        assert_eq!(require_approver(user(1)), Ok(()));
        assert_eq!(require_owner(user(2)), Err(NOT_AN_OWNER.to_string()));
        assert_eq!(require_approver(user(2)), Ok(()));
        assert_eq!(require_owner(user(3)), Err(NOT_AN_OWNER.to_string()));
        assert_eq!(require_approver(user(3)), Err(NOT_AN_APPROVER.to_string()));
        assert_eq!(require_owner(user(4)), Err(NOT_A_MEMBER.to_string()));
        assert_eq!(require_approver(user(4)), Err(NOT_A_MEMBER.to_string()));
    }
}
