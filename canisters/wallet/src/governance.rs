//! The members and the policy, and the actions that change them: adding,
//! removing and re-roling members, and setting the policy. Pure: `storage`
//! loads a `Governance`, the caller applies an action, and `storage` saves it.
//!
//! Every change keeps the wallet governable: at least one owner, and enough
//! owners and approvers to meet every rule's quorum.

use crate::policy::{self, check_governance};
use crate::storage::StoredMember;
use crate::{Action, Policy, Role, WalletError};
use candid::Principal;
use std::collections::BTreeMap;

/// At most this many members, so every list of them stays small.
pub const MAX_MEMBERS: usize = 64;
/// The longest member label, in bytes of UTF-8.
pub const MAX_LABEL_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Governance {
    pub members: BTreeMap<Principal, StoredMember>,
    pub policy: Policy,
}

fn check_label(label: &str) -> Result<(), WalletError> {
    if label.trim().is_empty() || label.len() > MAX_LABEL_BYTES {
        return Err(WalletError::InvalidArgument(format!(
            "a label needs 1 to {MAX_LABEL_BYTES} bytes"
        )));
    }
    Ok(())
}

impl Governance {
    pub fn role(&self, id: &Principal) -> Option<Role> {
        self.members.get(id).map(|member| member.role)
    }

    /// Whether `id` is a member who can approve.
    pub fn can_approve(&self, id: &Principal) -> bool {
        self.role(id).is_some_and(policy::approves)
    }

    /// Checks that `action` could be applied now, without changing anything.
    pub fn check(&self, action: &Action, added_by: Principal, now: u64) -> Result<(), WalletError> {
        self.clone().apply(action, added_by, now)
    }

    /// Applies an action that passed its policy. On error nothing changes.
    /// `added_by` is recorded on a member it adds.
    pub fn apply(
        &mut self,
        action: &Action,
        added_by: Principal,
        now: u64,
    ) -> Result<(), WalletError> {
        let mut next = self.clone();
        match action {
            Action::AddMember { id, role, label } => {
                if *id == Principal::anonymous() {
                    return Err(WalletError::InvalidArgument(
                        "the anonymous principal cannot be a member".to_string(),
                    ));
                }
                if self.members.contains_key(id) {
                    return Err(WalletError::InvalidArgument(format!(
                        "{id} is already a member"
                    )));
                }
                check_label(label)?;
                if self.members.len() >= MAX_MEMBERS {
                    return Err(WalletError::PolicyViolation(format!(
                        "a wallet has at most {MAX_MEMBERS} members"
                    )));
                }
                next.members.insert(
                    *id,
                    StoredMember {
                        role: *role,
                        label: label.clone(),
                        added_at_ns: now,
                        added_by: Some(added_by),
                    },
                );
            }
            Action::RemoveMember(id) => {
                if next.members.remove(id).is_none() {
                    return Err(WalletError::NotFound(format!("member {id}")));
                }
            }
            Action::ChangeRole { id, role } => {
                let Some(member) = next.members.get_mut(id) else {
                    return Err(WalletError::NotFound(format!("member {id}")));
                };
                if member.role == *role {
                    return Err(WalletError::InvalidArgument(format!(
                        "{id} already has the role {role:?}"
                    )));
                }
                member.role = *role;
            }
            Action::SetPolicy(policy) => {
                policy::validate(policy).map_err(WalletError::InvalidArgument)?;
                next.policy = policy.clone();
            }
            // Transfers, extensions and upgrades arrive in later steps.
            Action::Transfer(_)
            | Action::InstallExtension { .. }
            | Action::UninstallExtension(_)
            | Action::Upgrade { .. } => {
                return Err(WalletError::InvalidArgument(format!(
                    "this release cannot execute {:?} actions yet",
                    action.kind()
                )));
            }
        }
        check_governance(
            next.members.values().map(|member| member.role),
            &next.policy,
        )
        .map_err(WalletError::PolicyViolation)?;
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::tests::valid_policy;
    use crate::policy::{DAY_NS, default_policy};
    use crate::{ActionKind, PolicyRule};
    use proptest::prelude::*;

    fn member(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn stored(role: Role) -> StoredMember {
        StoredMember {
            role,
            label: "device".to_string(),
            added_at_ns: 0,
            added_by: None,
        }
    }

    fn one_owner() -> Governance {
        Governance {
            members: BTreeMap::from([(member(1), stored(Role::Owner))]),
            policy: default_policy(),
        }
    }

    fn quorum_two() -> Policy {
        let mut policy = default_policy();
        for rule in &mut policy.rules {
            rule.quorum = 2;
        }
        policy
    }

    #[test]
    fn adds_a_member_with_who_added_it() {
        let mut governance = one_owner();
        let add = Action::AddMember {
            id: member(2),
            role: Role::Approver,
            label: "laptop".to_string(),
        };
        governance.apply(&add, member(1), 7).unwrap();
        assert_eq!(
            governance.members[&member(2)],
            StoredMember {
                role: Role::Approver,
                label: "laptop".to_string(),
                added_at_ns: 7,
                added_by: Some(member(1)),
            }
        );
        assert!(governance.can_approve(&member(2)));
        assert!(matches!(
            governance.apply(&add, member(1), 8),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("already a member")
        ));
    }

    #[test]
    fn refuses_bad_members() {
        let governance = one_owner();
        let add = |id, label: &str| Action::AddMember {
            id,
            role: Role::Viewer,
            label: label.to_string(),
        };
        for action in [
            add(Principal::anonymous(), "x"),
            add(member(2), " "),
            add(member(2), &"x".repeat(MAX_LABEL_BYTES + 1)),
        ] {
            assert!(matches!(
                governance.check(&action, member(1), 0),
                Err(WalletError::InvalidArgument(_))
            ));
        }
        assert_eq!(
            governance.check(&add(member(2), &"x".repeat(MAX_LABEL_BYTES)), member(1), 0),
            Ok(())
        );

        let mut full = one_owner();
        for seed in 2..=MAX_MEMBERS as u8 {
            full.members.insert(member(seed), stored(Role::Viewer));
        }
        assert!(matches!(
            full.check(&add(member(200), "x"), member(1), 0),
            Err(WalletError::PolicyViolation(_))
        ));
    }

    #[test]
    fn never_removes_or_demotes_the_last_owner() {
        let governance = one_owner();
        for action in [
            Action::RemoveMember(member(1)),
            Action::ChangeRole {
                id: member(1),
                role: Role::Approver,
            },
        ] {
            assert!(matches!(
                governance.check(&action, member(1), 0),
                Err(WalletError::PolicyViolation(reason)) if reason.contains("at least one owner")
            ));
        }
        assert!(matches!(
            governance.check(&Action::RemoveMember(member(2)), member(1), 0),
            Err(WalletError::NotFound(_))
        ));
    }

    #[test]
    fn a_quorum_needs_enough_members_to_meet_it() {
        let mut governance = one_owner();
        assert!(matches!(
            governance.check(&Action::SetPolicy(quorum_two()), member(1), 0),
            Err(WalletError::PolicyViolation(reason)) if reason.contains("a quorum of 2")
        ));
        governance.members.insert(member(2), stored(Role::Approver));
        governance
            .apply(&Action::SetPolicy(quorum_two()), member(1), 0)
            .unwrap();
        // With a quorum of two, the approver can no longer be removed.
        assert!(matches!(
            governance.check(&Action::RemoveMember(member(2)), member(1), 0),
            Err(WalletError::PolicyViolation(_))
        ));
    }

    #[test]
    fn refuses_an_invalid_policy_and_unsupported_actions() {
        let governance = one_owner();
        let mut invalid = default_policy();
        invalid.rules[0] = PolicyRule {
            kind: ActionKind::Transfer,
            quorum: 1,
            timelock_ns: 8 * DAY_NS,
            daily_limits: vec![],
        };
        assert!(matches!(
            governance.check(&Action::SetPolicy(invalid), member(1), 0),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("SetPolicy rule")
        ));
        assert!(matches!(
            governance.check(&Action::UninstallExtension(member(3)), member(1), 0),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("cannot execute UninstallExtension")
        ));
    }

    fn role() -> impl Strategy<Value = Role> {
        prop_oneof![Just(Role::Owner), Just(Role::Approver), Just(Role::Viewer)]
    }

    fn change() -> impl Strategy<Value = Action> {
        let id = (1u8..=6).prop_map(member);
        prop_oneof![
            (id.clone(), role()).prop_map(|(id, role)| Action::AddMember {
                id,
                role,
                label: "device".to_string(),
            }),
            id.clone().prop_map(Action::RemoveMember),
            (id, role()).prop_map(|(id, role)| Action::ChangeRole { id, role }),
            valid_policy().prop_map(Action::SetPolicy),
        ]
    }

    proptest! {
        /// Whatever membership and policy changes are tried, in any order,
        /// the wallet always keeps an owner and enough members who can
        /// approve to meet every quorum, and a refused change changes
        /// nothing.
        #[test]
        fn the_wallet_stays_governable(changes in prop::collection::vec(change(), 1..30)) {
            let mut governance = one_owner();
            for change in changes {
                let before = governance.clone();
                let checked = governance.check(&change, member(1), 0);
                let applied = governance.apply(&change, member(1), 0);
                prop_assert_eq!(&checked, &applied);
                if applied.is_err() {
                    prop_assert_eq!(&governance, &before);
                }
                let roles: Vec<Role> = governance.members.values().map(|m| m.role).collect();
                prop_assert!(roles.contains(&Role::Owner));
                let approvers = roles.iter().filter(|role| policy::approves(**role)).count();
                for rule in &governance.policy.rules {
                    prop_assert!(usize::from(rule.quorum) <= approvers);
                }
                prop_assert_eq!(policy::validate(&governance.policy), Ok(()));
            }
        }
    }
}
