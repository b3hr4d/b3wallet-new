//! Who may call what. A method that not everyone may call names one of these
//! guards. ic-cdk runs the guard before it decodes the arguments, and a refused
//! call is rejected with the guard's message, so a caller the wallet refuses
//! never gets it to decode anything.
//!
//! Anyone may call `claim`, `status` and `http_request`; once the wallet is
//! claimed, also `whoami` and `request_join`. Every other method is for
//! members only, except `ext_request`, which will have its own guard for
//! registered extensions.

use crate::storage;
use candid::Principal;

pub const NOT_CLAIMED: &str = "the wallet is not claimed yet";
pub const NOT_A_MEMBER: &str = "the caller is not a member of this wallet";

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

fn require_member(caller: Principal) -> Result<(), String> {
    claimed()?;
    match storage::member(caller) {
        Some(_) => Ok(()),
        None => Err(NOT_A_MEMBER.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use crate::storage::StoredMember;

    fn owner() -> Principal {
        Principal::self_authenticating([1; 32])
    }

    fn add(id: Principal) {
        storage::insert_member(
            id,
            StoredMember {
                role: Role::Viewer,
                label: "test".to_string(),
                added_at_ns: 0,
                added_by: None,
            },
        );
    }

    #[test]
    fn before_the_claim_everyone_is_refused() {
        assert_eq!(claimed(), Err(NOT_CLAIMED.to_string()));
        assert_eq!(require_member(owner()), Err(NOT_CLAIMED.to_string()));
        assert_eq!(
            require_member(Principal::anonymous()),
            Err(NOT_CLAIMED.to_string())
        );
    }

    #[test]
    fn after_the_claim_only_members_pass() {
        add(owner());
        assert_eq!(claimed(), Ok(()));
        assert_eq!(require_member(owner()), Ok(()));
        assert_eq!(
            require_member(Principal::self_authenticating([2; 32])),
            Err(NOT_A_MEMBER.to_string())
        );
        assert_eq!(
            require_member(Principal::anonymous()),
            Err(NOT_A_MEMBER.to_string())
        );
    }
}
