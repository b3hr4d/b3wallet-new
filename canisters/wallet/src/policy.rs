//! The approval policy: for each kind of action, how many approvals it needs
//! and how long it waits once it has them. Pure: no storage and no clock.

use crate::assets;
use crate::{ActionKind, Chain, Policy, PolicyRule, Role};
use candid::Nat;

pub const HOUR_NS: u64 = 60 * 60 * 1_000_000_000;
pub const DAY_NS: u64 = 24 * HOUR_NS;
/// The longest timelock a rule may set.
pub const MAX_TIMELOCK_NS: u64 = 90 * DAY_NS;

/// The policy a wallet starts with: one approval for everything, and the
/// timelocks `docs/design.md` recommends.
pub fn default_policy() -> Policy {
    let rule = |kind, timelock_ns| PolicyRule {
        kind,
        quorum: 1,
        timelock_ns,
        daily_limits: vec![],
    };
    Policy {
        rules: vec![
            rule(ActionKind::Transfer, 0),
            rule(ActionKind::AddMember, 0),
            rule(ActionKind::RemoveMember, DAY_NS),
            rule(ActionKind::ChangeRole, DAY_NS),
            rule(ActionKind::InstallExtension, 0),
            rule(ActionKind::UninstallExtension, 0),
            rule(ActionKind::SetPolicy, 7 * DAY_NS),
            rule(ActionKind::Upgrade, 7 * DAY_NS),
        ],
    }
}

/// Checks a policy's shape. A stored policy always passes it.
pub fn validate(policy: &Policy) -> Result<(), String> {
    for kind in ActionKind::ALL {
        let count = policy.rules.iter().filter(|rule| rule.kind == kind).count();
        if count != 1 {
            return Err(format!(
                "the policy needs exactly one {kind:?} rule, not {count}"
            ));
        }
    }
    for rule in &policy.rules {
        let kind = rule.kind;
        if rule.quorum == 0 {
            return Err(format!("the {kind:?} rule needs a quorum of at least 1"));
        }
        if rule.timelock_ns > MAX_TIMELOCK_NS {
            return Err(format!(
                "the {kind:?} rule's timelock is longer than {} days",
                MAX_TIMELOCK_NS / DAY_NS
            ));
        }
        if kind != ActionKind::Transfer && !rule.daily_limits.is_empty() {
            return Err(format!(
                "only the Transfer rule has daily limits, not {kind:?}"
            ));
        }
        for (index, limit) in rule.daily_limits.iter().enumerate() {
            assets::check(&limit.chain).map_err(|reason| format!("a daily limit: {reason}"))?;
            if rule.daily_limits[..index]
                .iter()
                .any(|earlier| earlier.chain == limit.chain)
            {
                return Err(format!("two daily limits for {:?}", limit.chain));
            }
        }
    }
    // Changing the policy must be as hard as anything the policy guards, or a
    // weaker SetPolicy rule would lower the other rules first.
    let set_policy = rule(policy, ActionKind::SetPolicy);
    for rule in &policy.rules {
        if rule.quorum > set_policy.quorum || rule.timelock_ns > set_policy.timelock_ns {
            return Err(format!(
                "the SetPolicy rule needs at least the quorum and timelock of the {:?} rule",
                rule.kind
            ));
        }
    }
    Ok(())
}

/// The rule for `kind` in a policy that passed `validate`.
pub fn rule(policy: &Policy, kind: ActionKind) -> &PolicyRule {
    policy
        .rules
        .iter()
        .find(|rule| rule.kind == kind)
        .expect("a valid policy has a rule for every kind")
}

/// Whether a member of this role can approve, and so counts toward a quorum.
pub fn approves(role: Role) -> bool {
    matches!(role, Role::Owner | Role::Approver)
}

/// Checks that members with these roles can still run the wallet under the
/// policy: someone can propose, and every rule's quorum can be met.
pub fn check_governance(
    roles: impl IntoIterator<Item = Role>,
    policy: &Policy,
) -> Result<(), String> {
    let (mut owners, mut approvers) = (0usize, 0usize);
    for role in roles {
        owners += usize::from(role == Role::Owner);
        approvers += usize::from(approves(role));
    }
    if owners == 0 {
        return Err("the wallet must keep at least one owner".to_string());
    }
    let quorum = policy
        .rules
        .iter()
        .map(|rule| rule.quorum)
        .max()
        .unwrap_or(1);
    if usize::from(quorum) > approvers {
        return Err(format!(
            "a quorum of {quorum} needs that many owners and approvers, and there would be {approvers}"
        ));
    }
    Ok(())
}

/// What an action needs before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terms {
    pub quorum: u8,
    pub timelock_ns: u64,
    /// Whether these are a transfer's terms under its chain's daily limit,
    /// lighter than its rule. `execute` checks the limit again.
    pub daily_limit: bool,
}

/// A transfer's chain and amount, and what transfers under the daily limit
/// spent on that chain in the last 24 hours.
pub struct Spend<'a> {
    pub chain: &'a Chain,
    pub amount: &'a Nat,
    pub spent: &'a Nat,
}

/// The terms for an action under `policy`. A transfer that stays within its
/// chain's daily limit needs only the proposer and no timelock; under a rule
/// that asks no more, it takes the rule's terms.
pub fn terms(policy: &Policy, kind: ActionKind, spend: Option<&Spend>) -> Terms {
    let rule = rule(policy, kind);
    if kind == ActionKind::Transfer
        && (rule.quorum > 1 || rule.timelock_ns > 0)
        && let Some(spend) = spend
        && let Some(limit) = rule
            .daily_limits
            .iter()
            .find(|limit| &limit.chain == spend.chain)
        && spend.spent.clone() + spend.amount.clone() <= limit.amount
    {
        return Terms {
            quorum: 1,
            timelock_ns: 0,
            daily_limit: true,
        };
    }
    Terms {
        quorum: rule.quorum,
        timelock_ns: rule.timelock_ns,
        daily_limit: false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::DailyLimit;
    use proptest::prelude::*;

    pub(crate) fn chain() -> impl Strategy<Value = Chain> {
        prop_oneof![
            Just(Chain::Icp),
            Just(Chain::Bitcoin(crate::BitcoinNetwork::Mainnet)),
            (1u64..3).prop_map(Chain::Evm),
        ]
    }

    /// Policies that pass `validate`: the SetPolicy rule takes the highest
    /// quorum and timelock.
    pub(crate) fn valid_policy() -> impl Strategy<Value = Policy> {
        let rule = (1u8..=4, 0..=3 * DAY_NS);
        let limits = prop::collection::btree_map(0u8..3, 0u64..1_000, 0..3);
        (prop::collection::vec(rule, 8), limits).prop_map(|(terms, limits)| {
            let quorum = terms.iter().map(|(quorum, _)| *quorum).max().unwrap();
            let timelock_ns = terms.iter().map(|(_, timelock)| *timelock).max().unwrap();
            let rules = ActionKind::ALL
                .iter()
                .zip(terms)
                .map(|(&kind, (rule_quorum, rule_timelock))| {
                    let set_policy = kind == ActionKind::SetPolicy;
                    PolicyRule {
                        kind,
                        quorum: if set_policy { quorum } else { rule_quorum },
                        timelock_ns: if set_policy {
                            timelock_ns
                        } else {
                            rule_timelock
                        },
                        daily_limits: if kind == ActionKind::Transfer {
                            limits
                                .iter()
                                .map(|(&chain, &amount)| DailyLimit {
                                    chain: Chain::Evm(chain.into()),
                                    amount: Nat::from(amount),
                                })
                                .collect()
                        } else {
                            vec![]
                        },
                    }
                })
                .collect();
            Policy { rules }
        })
    }

    fn with_rule(mut policy: Policy, kind: ActionKind, change: impl Fn(&mut PolicyRule)) -> Policy {
        change(
            policy
                .rules
                .iter_mut()
                .find(|rule| rule.kind == kind)
                .unwrap(),
        );
        policy
    }

    #[test]
    fn the_default_policy_is_valid_for_the_first_owner() {
        let policy = default_policy();
        assert_eq!(validate(&policy), Ok(()));
        assert_eq!(check_governance([Role::Owner], &policy), Ok(()));
    }

    #[test]
    fn a_policy_needs_one_rule_per_kind() {
        let mut missing = default_policy();
        missing
            .rules
            .retain(|rule| rule.kind != ActionKind::Upgrade);
        assert!(
            validate(&missing)
                .unwrap_err()
                .contains("one Upgrade rule, not 0")
        );

        let mut doubled = default_policy();
        doubled.rules.push(doubled.rules[0].clone());
        assert!(
            validate(&doubled)
                .unwrap_err()
                .contains("one Transfer rule, not 2")
        );
    }

    #[test]
    fn a_rule_needs_a_quorum_and_a_bounded_timelock() {
        let zero = with_rule(default_policy(), ActionKind::AddMember, |rule| {
            rule.quorum = 0
        });
        assert!(
            validate(&zero)
                .unwrap_err()
                .contains("quorum of at least 1")
        );

        let long = with_rule(default_policy(), ActionKind::Upgrade, |rule| {
            rule.timelock_ns = MAX_TIMELOCK_NS + 1
        });
        assert!(validate(&long).unwrap_err().contains("longer than 90 days"));
    }

    #[test]
    fn only_transfers_have_daily_limits_and_one_per_chain() {
        let limit = DailyLimit {
            chain: Chain::Icp,
            amount: Nat::from(5u8),
        };
        let elsewhere = with_rule(default_policy(), ActionKind::Upgrade, |rule| {
            rule.daily_limits = vec![limit.clone()]
        });
        assert!(validate(&elsewhere).unwrap_err().contains("not Upgrade"));

        let twice = with_rule(default_policy(), ActionKind::Transfer, |rule| {
            rule.daily_limits = vec![limit.clone(), limit.clone()]
        });
        assert!(
            validate(&twice)
                .unwrap_err()
                .contains("two daily limits for Icp")
        );
    }

    #[test]
    fn a_wallet_keeps_an_owner_and_enough_approvers() {
        let policy = default_policy();
        assert!(
            check_governance([Role::Approver, Role::Viewer], &policy)
                .unwrap_err()
                .contains("at least one owner")
        );
        assert!(check_governance(std::iter::empty(), &policy).is_err());

        let two = with_rule(policy, ActionKind::SetPolicy, |rule| rule.quorum = 2);
        assert_eq!(
            check_governance([Role::Owner, Role::Approver], &two),
            Ok(())
        );
        assert!(
            check_governance([Role::Owner, Role::Viewer], &two)
                .unwrap_err()
                .contains("a quorum of 2")
        );
    }

    #[test]
    fn a_transfer_may_reach_the_daily_limit_but_not_pass_it() {
        let policy = with_rule(default_policy(), ActionKind::Transfer, |rule| {
            rule.quorum = 2;
            rule.timelock_ns = DAY_NS;
            rule.daily_limits = vec![DailyLimit {
                chain: Chain::Icp,
                amount: Nat::from(100u8),
            }];
        });
        let policy = with_rule(policy, ActionKind::SetPolicy, |rule| rule.quorum = 2);
        assert_eq!(validate(&policy), Ok(()));
        let terms_for = |chain: &Chain, amount: u8, spent: u8| {
            let (amount, spent) = (Nat::from(amount), Nat::from(spent));
            let spend = Spend {
                chain,
                amount: &amount,
                spent: &spent,
            };
            terms(&policy, ActionKind::Transfer, Some(&spend))
        };
        let fast = Terms {
            quorum: 1,
            timelock_ns: 0,
            daily_limit: true,
        };
        let rule = Terms {
            quorum: 2,
            timelock_ns: DAY_NS,
            daily_limit: false,
        };
        assert_eq!(terms_for(&Chain::Icp, 60, 40), fast);
        assert_eq!(terms_for(&Chain::Icp, 61, 40), rule);
        assert_eq!(terms_for(&Chain::Icp, 0, 101), rule);
        // A chain without a limit always takes the rule.
        assert_eq!(terms_for(&Chain::Evm(1), 1, 0), rule);
    }

    #[test]
    fn under_a_rule_that_asks_no_more_a_limit_changes_nothing() {
        let policy = with_rule(default_policy(), ActionKind::Transfer, |rule| {
            rule.daily_limits = vec![DailyLimit {
                chain: Chain::Icp,
                amount: Nat::from(100u8),
            }];
        });
        let (amount, spent) = (Nat::from(1u8), Nat::from(0u8));
        let spend = Spend {
            chain: &Chain::Icp,
            amount: &amount,
            spent: &spent,
        };
        assert_eq!(
            terms(&policy, ActionKind::Transfer, Some(&spend)),
            Terms {
                quorum: 1,
                timelock_ns: 0,
                daily_limit: false,
            }
        );
    }

    #[test]
    fn a_daily_limit_names_the_icp_ledger_as_icp_and_a_canister_as_icrc() {
        for chain in [
            Chain::Icrc(crate::assets::ICP_LEDGER),
            Chain::Icrc(candid::Principal::self_authenticating([1; 32])),
        ] {
            let policy = with_rule(default_policy(), ActionKind::Transfer, |rule| {
                rule.daily_limits = vec![DailyLimit {
                    chain: chain.clone(),
                    amount: Nat::from(5u8),
                }];
            });
            assert!(
                validate(&policy).unwrap_err().starts_with("a daily limit:"),
                "{chain:?}"
            );
        }
    }

    proptest! {
        /// Lowering the SetPolicy rule below any other rule makes the policy
        /// invalid, so the policy can never be its own weakest rule.
        #[test]
        fn set_policy_is_never_weaker_than_another_rule(
            policy in valid_policy(),
            other in 0usize..8,
            lower_quorum in any::<bool>(),
        ) {
            prop_assert_eq!(validate(&policy), Ok(()));
            let other = policy.rules[other].clone();
            prop_assume!(other.kind != ActionKind::SetPolicy);
            let weaker = with_rule(policy, ActionKind::SetPolicy, |rule| {
                if lower_quorum {
                    rule.quorum = other.quorum.saturating_sub(1).max(1);
                    rule.timelock_ns = other.timelock_ns;
                } else {
                    rule.quorum = other.quorum;
                    rule.timelock_ns = other.timelock_ns.saturating_sub(1);
                }
            });
            let still_dominant = weaker.rules.iter().all(|rule| {
                let set_policy = super::rule(&weaker, ActionKind::SetPolicy);
                rule.quorum <= set_policy.quorum && rule.timelock_ns <= set_policy.timelock_ns
            });
            prop_assert_eq!(validate(&weaker).is_ok(), still_dominant);
        }

        /// A transfer skips its rule exactly when the rule asks for more than
        /// the proposer and its chain has a daily limit that the amount, added
        /// to what was spent, does not pass. Nothing else ever gets lighter
        /// terms than its rule.
        #[test]
        fn only_transfers_within_the_daily_limit_skip_their_rule(
            policy in valid_policy(),
            kind_index in 0usize..8,
            chain in chain(),
            amount in 0u64..1_500,
            spent in 0u64..1_500,
        ) {
            let kind = ActionKind::ALL[kind_index];
            let (amount, spent) = (Nat::from(amount), Nat::from(spent));
            let spend = Spend { chain: &chain, amount: &amount, spent: &spent };
            let rule = rule(&policy, kind).clone();
            let within_limit = kind == ActionKind::Transfer
                && (rule.quorum > 1 || rule.timelock_ns > 0)
                && rule.daily_limits.iter().any(|limit| {
                    limit.chain == chain && spent.clone() + amount.clone() <= limit.amount
                });
            let by_rule = Terms { quorum: rule.quorum, timelock_ns: rule.timelock_ns, daily_limit: false };
            let expected = if within_limit {
                Terms { quorum: 1, timelock_ns: 0, daily_limit: true }
            } else {
                by_rule
            };
            prop_assert_eq!(terms(&policy, kind, Some(&spend)), expected);
            prop_assert_eq!(terms(&policy, kind, None), by_rule);
        }

        /// Spending more never turns a transfer that needs its rule into one
        /// that does not.
        #[test]
        fn spending_more_never_lightens_the_terms(
            policy in valid_policy(),
            chain in chain(),
            amount in 0u64..1_500,
            spent in 0u64..1_500,
            more in 0u64..1_500,
        ) {
            let amount = Nat::from(amount);
            let (spent, later) = (Nat::from(spent), Nat::from(spent + more));
            let before = terms(&policy, ActionKind::Transfer, Some(&Spend { chain: &chain, amount: &amount, spent: &spent }));
            let after = terms(&policy, ActionKind::Transfer, Some(&Spend { chain: &chain, amount: &amount, spent: &later }));
            prop_assert!(after.quorum >= before.quorum && after.timelock_ns >= before.timelock_ns);
        }
    }
}
