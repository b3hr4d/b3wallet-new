//! The life of a pending action: proposed, approved up to its quorum, held
//! for its timelock, then executed; or rejected, or expired. Pure: the caller
//! passes the time and stores the result.
//!
//! An action waits for approvals until `expires_at_ns`. Once it has its
//! quorum the timelock starts, and the action may run from
//! `executable_after_ns` until a new `expires_at_ns`, one execution window
//! later. An action past its expiry is expired, whether or not that was
//! written down yet.
//!
//! A transfer stays open past its expiry while one of its ledger calls is in
//! flight or ended without the wallet learning whether it ran, until the
//! ledger no longer finds it: only another attempt can tell.

use crate::policy::{DAY_NS, HOUR_NS, Terms};
use crate::transfers::{DEDUP_WINDOW_NS, Reply, STALE_CALL_NS};
use crate::{
    AccountId, Action, ActionId, ActionState, Chain, Outcome, Pending, TxReceipt, WalletError,
};
use candid::{CandidType, Nat, Principal};
use serde::Deserialize;

/// How long a member's proposal waits for its quorum.
pub const APPROVAL_WINDOW_NS: u64 = 7 * DAY_NS;
/// How long a join request waits: both screens show its code at once.
pub const JOIN_WINDOW_NS: u64 = HOUR_NS;
/// How long an action with its quorum may run once its timelock has passed.
pub const EXECUTION_WINDOW_NS: u64 = 7 * DAY_NS;
/// At most this many proposals by members are open at once.
pub const MAX_OPEN_ACTIONS: usize = 64;
/// At most this many join requests are open at once, apart from the
/// proposals, so strangers cannot crowd the members out.
pub const MAX_OPEN_JOIN_REQUESTS: usize = 8;

/// A pending action as stored, under its id.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredPending {
    pub action: Action,
    pub proposed_by: Principal,
    pub proposed_at_ns: u64,
    /// Members who approved, in order, without repeats.
    pub approvals: Vec<Principal>,
    pub quorum: u8,
    pub timelock_ns: u64,
    /// Zero until the action has its quorum.
    pub executable_after_ns: u64,
    pub expires_at_ns: u64,
    pub state: ActionState,
    /// Whether a transfer's chain's daily limit released it without its
    /// rule's quorum and timelock. Absent from actions stored before step 8.
    pub daily_limit: Option<bool>,
    /// A transfer's latest ledger call, once `execute` started one.
    pub transfer: Option<TransferAttempt>,
}

/// What the wallet knows of a transfer's calls to the ledger.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransferAttempt {
    /// The `created_at_time` of the latest call, by which the ledger finds a
    /// repeated transfer.
    pub created_at_ns: u64,
    /// When the call in flight started; `None` once it returned.
    pub in_flight_since_ns: Option<u64>,
    /// Whether a call with `created_at_ns` may have run without the wallet
    /// learning how it ended. The next call then reuses `created_at_ns`.
    pub unknown: bool,
}

impl StoredPending {
    /// A new action. `approvals` holds the proposer for a member's proposal,
    /// and nobody for a join request, whose proposer is not a member yet.
    pub fn new(
        action: Action,
        proposed_by: Principal,
        approvals: Vec<Principal>,
        terms: Terms,
        approval_window_ns: u64,
        now: u64,
    ) -> Self {
        let mut pending = StoredPending {
            action,
            proposed_by,
            proposed_at_ns: now,
            approvals,
            quorum: terms.quorum,
            timelock_ns: terms.timelock_ns,
            executable_after_ns: 0,
            expires_at_ns: now.saturating_add(approval_window_ns),
            state: ActionState::Waiting,
            daily_limit: Some(terms.daily_limit),
            transfer: None,
        };
        pending.release_at_quorum(now);
        pending
    }

    /// A join request is an `AddMember` of its own proposer, which no member
    /// can propose, since it would add a member who is already one.
    pub fn is_join_request(&self) -> bool {
        matches!(self.action, Action::AddMember { id, .. } if id == self.proposed_by)
    }

    /// Waiting or ready, and not past its expiry at `now`.
    pub fn is_open(&self, now: u64) -> bool {
        matches!(self.state, ActionState::Waiting | ActionState::Ready) && now < self.open_until()
    }

    /// The expiry, or for a transfer whose latest call may have run, the end
    /// of the window in which the ledger finds it, if that is later.
    fn open_until(&self) -> u64 {
        match &self.transfer {
            Some(attempt) if attempt.unknown || attempt.in_flight_since_ns.is_some() => self
                .expires_at_ns
                .max(attempt.created_at_ns.saturating_add(DEDUP_WINDOW_NS)),
            _ => self.expires_at_ns,
        }
    }

    /// Whether a ledger call of this transfer may have run unseen.
    pub fn may_have_sent(&self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|attempt| attempt.unknown || attempt.in_flight_since_ns.is_some())
    }

    fn release_at_quorum(&mut self, now: u64) {
        if self.approvals.len() >= usize::from(self.quorum) {
            self.state = ActionState::Ready;
            self.executable_after_ns = now.saturating_add(self.timelock_ns);
            self.expires_at_ns = self.executable_after_ns.saturating_add(EXECUTION_WINDOW_NS);
        }
    }

    /// Writes down an expiry that has passed.
    pub fn expire(&mut self, now: u64) {
        if matches!(self.state, ActionState::Waiting | ActionState::Ready)
            && now >= self.open_until()
        {
            self.state = ActionState::Expired;
        }
    }

    fn check_open(&mut self, id: ActionId, now: u64) -> Result<(), WalletError> {
        self.expire(now);
        let state = match self.state {
            ActionState::Waiting | ActionState::Ready => return Ok(()),
            ActionState::Executed(_) => "executed",
            ActionState::Rejected(_) => "rejected",
            ActionState::Expired => "expired",
            ActionState::Failed(_) => "failed",
        };
        Err(WalletError::InvalidArgument(format!(
            "action {id} is already {state}"
        )))
    }

    /// Adds an approval by a member who may approve. Approving twice changes
    /// nothing; approving an action that already has its quorum is refused.
    pub fn approve(
        &mut self,
        id: ActionId,
        approver: Principal,
        now: u64,
    ) -> Result<(), WalletError> {
        self.check_open(id, now)?;
        if self.approvals.contains(&approver) {
            return Ok(());
        }
        if self.state == ActionState::Ready {
            return Err(WalletError::InvalidArgument(format!(
                "action {id} already has its quorum"
            )));
        }
        self.approvals.push(approver);
        self.release_at_quorum(now);
        Ok(())
    }

    /// Rejects an open action. A transfer that may have been sent cannot be
    /// rejected: only executing it again tells whether it was.
    pub fn reject(&mut self, id: ActionId, by: Principal, now: u64) -> Result<(), WalletError> {
        self.check_open(id, now)?;
        if self.may_have_sent() {
            return Err(WalletError::InvalidArgument(format!(
                "action {id} may have reached the ledger; execute it again to find out"
            )));
        }
        self.state = ActionState::Rejected(by);
        Ok(())
    }

    /// Checks that the action may run now: it has its quorum, its timelock
    /// has passed, and it has not expired.
    pub fn check_executable(&mut self, id: ActionId, now: u64) -> Result<(), WalletError> {
        self.check_open(id, now)?;
        if self.state == ActionState::Waiting {
            return Err(WalletError::QuorumNotMet {
                approvals: u8::try_from(self.approvals.len()).unwrap_or(u8::MAX),
                quorum: self.quorum,
            });
        }
        if now < self.executable_after_ns {
            return Err(WalletError::TimelockActive {
                executable_after_ns: self.executable_after_ns,
            });
        }
        Ok(())
    }

    /// Whether the next ledger call of this transfer retries one that may
    /// have run, and so must not be checked again like a new one. Refuses
    /// while a call is in flight.
    pub fn transfer_retry(&self, id: ActionId, now: u64) -> Result<bool, WalletError> {
        let Some(attempt) = &self.transfer else {
            return Ok(false);
        };
        match attempt.in_flight_since_ns {
            Some(since) if now < since.saturating_add(STALE_CALL_NS) => Err(
                WalletError::InvalidArgument(format!("a ledger call for action {id} is in flight")),
            ),
            // A call that never returned to the wallet's code may have run.
            Some(_) => Ok(true),
            None => Ok(attempt.unknown),
        }
    }

    /// Starts a ledger call of this transfer and returns its
    /// `created_at_time`: the earlier one when an earlier call may have run,
    /// so the ledger finds it, and otherwise one from `fresh`, which no other
    /// call has. Call `transfer_retry` first.
    pub fn begin_transfer(&mut self, now: u64, fresh: impl FnOnce() -> u64) -> u64 {
        let retry = self
            .transfer
            .as_ref()
            .filter(|attempt| attempt.unknown || attempt.in_flight_since_ns.is_some());
        let attempt = match retry {
            Some(earlier) => TransferAttempt {
                created_at_ns: earlier.created_at_ns,
                in_flight_since_ns: Some(now),
                unknown: true,
            },
            None => TransferAttempt {
                created_at_ns: fresh(),
                in_flight_since_ns: Some(now),
                unknown: false,
            },
        };
        let created_at_ns = attempt.created_at_ns;
        self.transfer = Some(attempt);
        created_at_ns
    }

    /// Records that the call in flight will not return to the wallet's code:
    /// its callback trapped. The ledger may have run it.
    pub fn lose_transfer_call(&mut self) {
        if let Some(attempt) = &mut self.transfer {
            attempt.in_flight_since_ns = None;
            attempt.unknown = true;
        }
    }

    /// Records how the call `begin_transfer` started ended, from `account` on
    /// `chain`. Returns the block of a transfer the ledger holds, which ends
    /// the action `Executed`; a refusal ends it `Failed`. When the transfer
    /// may still be sent, the action stays ready and the error says why.
    pub fn end_transfer(
        &mut self,
        reply: Reply,
        account: AccountId,
        chain: Chain,
        now: u64,
    ) -> Result<Option<Nat>, WalletError> {
        let Some(attempt) = &mut self.transfer else {
            return Err(WalletError::InvalidArgument(
                "no ledger call was started".to_string(),
            ));
        };
        attempt.in_flight_since_ns = None;
        match reply {
            Reply::Sent(block) => {
                self.state = ActionState::Executed(Outcome::Sent(TxReceipt {
                    account,
                    chain,
                    tx_id: block.0.to_str_radix(10),
                    at_ns: now,
                }));
                Ok(Some(block))
            }
            // The refusal may have come before the ledger looked for the
            // earlier call, so that one may still have been sent.
            Reply::Refused(reason) if attempt.unknown => Err(WalletError::ChainError(format!(
                "{reason}; an earlier attempt may have been sent, so execute it again to find out"
            ))),
            Reply::Refused(reason) => {
                self.state = ActionState::Failed(reason);
                Ok(None)
            }
            Reply::NotRun(reason) => Err(WalletError::ChainError(reason)),
            Reply::Unknown(reason) => {
                attempt.unknown = true;
                Err(WalletError::ChainError(format!(
                    "{reason}; the ledger may have sent it, so execute it again to find out"
                )))
            }
            Reply::TooOld if attempt.unknown => {
                self.state = ActionState::Failed(format!(
                    "the ledger no longer finds transfers created at {} ns, so it cannot tell \
                     whether an earlier attempt was sent; check the ledger for it",
                    attempt.created_at_ns
                ));
                Ok(None)
            }
            // No earlier call ran, so the next one starts afresh.
            Reply::TooOld => {
                self.transfer = None;
                Err(WalletError::ChainError(
                    "the ledger refused the call as too old; execute it again".to_string(),
                ))
            }
        }
    }

    /// Records how an execution that `check_executable` allowed ended.
    pub fn finish(&mut self, result: Result<Outcome, String>) {
        self.state = match result {
            Ok(outcome) => ActionState::Executed(outcome),
            Err(reason) => ActionState::Failed(reason),
        };
    }

    /// Drops the approval of someone who can no longer approve, while the
    /// action still waits for its quorum.
    pub fn drop_approval(&mut self, member: Principal) {
        if self.state == ActionState::Waiting {
            self.approvals.retain(|approver| *approver != member);
        }
    }

    /// The action as the interface shows it at `now`.
    pub fn view(&self, id: ActionId, now: u64) -> Pending {
        let mut current = self.clone();
        current.expire(now);
        let expires_at_ns = current.open_until();
        Pending {
            id,
            action: current.action,
            proposed_by: current.proposed_by,
            proposed_at_ns: current.proposed_at_ns,
            approvals: current.approvals,
            quorum: current.quorum,
            executable_after_ns: current.executable_after_ns,
            expires_at_ns,
            state: current.state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use proptest::prelude::*;

    const ID: ActionId = 4;
    const T0: u64 = 1_000 * DAY_NS;

    fn member(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn add_member() -> Action {
        Action::AddMember {
            id: member(9),
            role: Role::Viewer,
            label: "phone".to_string(),
        }
    }

    fn proposal(quorum: u8, timelock_ns: u64) -> StoredPending {
        StoredPending::new(
            add_member(),
            member(1),
            vec![member(1)],
            Terms {
                quorum,
                timelock_ns,
                daily_limit: false,
            },
            APPROVAL_WINDOW_NS,
            T0,
        )
    }

    #[test]
    fn a_quorum_of_one_is_ready_at_once() {
        let pending = proposal(1, 0);
        assert_eq!(pending.state, ActionState::Ready);
        assert_eq!(pending.executable_after_ns, T0);
        assert_eq!(pending.expires_at_ns, T0 + EXECUTION_WINDOW_NS);
    }

    #[test]
    fn the_timelock_starts_at_the_quorum() {
        let mut pending = proposal(2, DAY_NS);
        assert_eq!(pending.state, ActionState::Waiting);
        assert_eq!(pending.executable_after_ns, 0);
        assert_eq!(
            pending.check_executable(ID, T0),
            Err(WalletError::QuorumNotMet {
                approvals: 1,
                quorum: 2
            })
        );

        let approved_at = T0 + HOUR_NS;
        assert_eq!(pending.approve(ID, member(2), approved_at), Ok(()));
        assert_eq!(pending.state, ActionState::Ready);
        assert_eq!(pending.executable_after_ns, approved_at + DAY_NS);
        assert_eq!(
            pending.check_executable(ID, approved_at + DAY_NS - 1),
            Err(WalletError::TimelockActive {
                executable_after_ns: approved_at + DAY_NS
            })
        );
        assert_eq!(pending.check_executable(ID, approved_at + DAY_NS), Ok(()));
    }

    #[test]
    fn approving_twice_changes_nothing_and_a_full_quorum_takes_no_more() {
        let mut pending = proposal(2, 0);
        let before = pending.clone();
        assert_eq!(pending.approve(ID, member(1), T0), Ok(()));
        assert_eq!(pending, before);

        pending.approve(ID, member(2), T0).unwrap();
        assert_eq!(pending.approve(ID, member(2), T0), Ok(()));
        assert!(matches!(
            pending.approve(ID, member(3), T0),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("already has its quorum")
        ));
        assert_eq!(pending.approvals, [member(1), member(2)]);
    }

    #[test]
    fn a_waiting_action_expires_and_says_so() {
        let mut pending = proposal(2, 0);
        let expiry = T0 + APPROVAL_WINDOW_NS;
        assert_eq!(pending.view(ID, expiry - 1).state, ActionState::Waiting);
        assert_eq!(pending.view(ID, expiry).state, ActionState::Expired);
        assert!(!pending.is_open(expiry));

        assert!(matches!(
            pending.approve(ID, member(2), expiry),
            Err(WalletError::InvalidArgument(reason)) if reason == "action 4 is already expired"
        ));
        assert_eq!(pending.state, ActionState::Expired);
    }

    #[test]
    fn a_ready_action_expires_one_window_after_its_timelock() {
        let mut pending = proposal(1, DAY_NS);
        let expiry = T0 + DAY_NS + EXECUTION_WINDOW_NS;
        assert_eq!(pending.expires_at_ns, expiry);
        assert_eq!(pending.check_executable(ID, expiry - 1), Ok(()));
        assert!(pending.check_executable(ID, expiry).is_err());
        assert_eq!(pending.state, ActionState::Expired);
    }

    #[test]
    fn a_dropped_approval_counts_no_more() {
        let mut pending = proposal(3, 0);
        pending.approve(ID, member(2), T0).unwrap();
        pending.drop_approval(member(2));
        assert_eq!(pending.approvals, [member(1)]);

        let mut ready = proposal(1, 0);
        ready.drop_approval(member(1));
        assert_eq!(ready.approvals, [member(1)]);
    }

    #[test]
    fn a_join_request_is_an_add_member_of_its_proposer() {
        let joiner = member(9);
        let request = StoredPending::new(
            add_member(),
            joiner,
            vec![],
            Terms {
                quorum: 1,
                timelock_ns: 0,
                daily_limit: false,
            },
            JOIN_WINDOW_NS,
            T0,
        );
        assert!(request.is_join_request());
        assert_eq!(request.state, ActionState::Waiting);
        assert_eq!(request.expires_at_ns, T0 + JOIN_WINDOW_NS);
        assert!(!proposal(1, 0).is_join_request());
    }

    #[derive(Clone, Debug)]
    enum Op {
        Approve(u8),
        Reject(u8),
        Execute,
        Wait(u64),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (1u8..=5).prop_map(Op::Approve),
            (1u8..=5).prop_map(Op::Reject),
            Just(Op::Execute),
            prop_oneof![0..=HOUR_NS, 0..=3 * DAY_NS, 0..=10 * DAY_NS].prop_map(Op::Wait),
        ]
    }

    proptest! {
        /// Whatever members do and however time passes, an action runs only
        /// with its quorum of distinct approvals, only once its timelock has
        /// passed since the quorum, and never after it expired; and once it
        /// is executed, rejected or expired, nothing changes it.
        #[test]
        fn an_action_runs_only_after_its_quorum_and_timelock(
            quorum in 1u8..=4,
            timelock_ns in 0..=3 * DAY_NS,
            ops in prop::collection::vec(op(), 1..40),
        ) {
            let mut pending = proposal(quorum, timelock_ns);
            let mut now = T0;
            let mut quorum_at = (quorum == 1).then_some(T0);
            for op in ops {
                let before = pending.clone();
                let was_open = pending.is_open(now);
                match op {
                    Op::Wait(ns) => now += ns,
                    Op::Approve(seed) => {
                        let result = pending.approve(ID, member(seed), now);
                        prop_assert_eq!(result.is_ok(), was_open && (
                            before.approvals.contains(&member(seed))
                                || before.state == ActionState::Waiting
                        ));
                        if quorum_at.is_none() && pending.state == ActionState::Ready {
                            quorum_at = Some(now);
                        }
                    }
                    Op::Reject(seed) => {
                        let result = pending.reject(ID, member(seed), now);
                        prop_assert_eq!(result.is_ok(), was_open);
                    }
                    Op::Execute => {
                        if pending.check_executable(ID, now).is_ok() {
                            let quorum_at = quorum_at.expect("ready actions have a quorum time");
                            prop_assert!(pending.approvals.len() >= usize::from(quorum));
                            prop_assert!(now >= quorum_at + timelock_ns);
                            prop_assert!(now < quorum_at + timelock_ns + EXECUTION_WINDOW_NS);
                            pending.finish(Ok(Outcome::Done));
                        }
                    }
                }
                let mut distinct = pending.approvals.clone();
                distinct.sort();
                distinct.dedup();
                prop_assert_eq!(distinct.len(), pending.approvals.len());
                if !was_open && !matches!(op, Op::Wait(_)) {
                    // A closed action only gets its expiry written down.
                    let mut expected = before.clone();
                    expected.expire(now);
                    prop_assert_eq!(&pending, &expected);
                }
            }
        }
    }

    fn transfer() -> StoredPending {
        StoredPending::new(
            Action::Transfer(crate::TransferRequest {
                from: 1,
                to: member(2).to_text(),
                amount: Nat::from(5u8),
                fee: None,
                memo: None,
            }),
            member(1),
            vec![member(1)],
            Terms {
                quorum: 1,
                timelock_ns: 0,
                daily_limit: true,
            },
            APPROVAL_WINDOW_NS,
            T0,
        )
    }

    fn end(
        pending: &mut StoredPending,
        reply: Reply,
        now: u64,
    ) -> Result<Option<Nat>, WalletError> {
        pending.end_transfer(reply, 1, Chain::Icp, now)
    }

    fn unknown() -> Reply {
        Reply::Unknown("timeout".to_string())
    }

    /// The fresh time of a call that must reuse an earlier one.
    fn no_fresh_time() -> u64 {
        panic!("a retry takes no fresh time")
    }

    #[test]
    fn a_sent_transfer_ends_executed_with_its_block() {
        let mut pending = transfer();
        assert_eq!(pending.daily_limit, Some(true));
        assert_eq!(pending.transfer_retry(ID, T0), Ok(false));
        // A fresh call takes the time it is given, not `now`.
        assert_eq!(pending.begin_transfer(T0, || T0 + 5), T0 + 5);
        let block = Nat::from(1_234u32);
        assert_eq!(
            end(&mut pending, Reply::Sent(block.clone()), T0 + 1),
            Ok(Some(block))
        );
        assert_eq!(
            pending.state,
            ActionState::Executed(Outcome::Sent(TxReceipt {
                account: 1,
                chain: Chain::Icp,
                tx_id: "1234".to_string(),
                at_ns: T0 + 1,
            }))
        );
    }

    #[test]
    fn one_call_at_a_time_and_one_that_never_returned_is_retried_as_it_was() {
        let mut pending = transfer();
        let created_at = pending.begin_transfer(T0, || T0);
        assert!(matches!(
            pending.transfer_retry(ID, T0 + STALE_CALL_NS - 1),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("in flight")
        ));
        assert_eq!(pending.transfer_retry(ID, T0 + STALE_CALL_NS), Ok(true));
        assert_eq!(
            pending.begin_transfer(T0 + STALE_CALL_NS, no_fresh_time),
            created_at
        );
    }

    #[test]
    fn after_an_unknown_outcome_every_retry_reuses_its_created_at_time() {
        let mut pending = transfer();
        let created_at = pending.begin_transfer(T0, || T0);
        assert!(matches!(
            end(&mut pending, unknown(), T0),
            Err(WalletError::ChainError(reason)) if reason.contains("execute it again")
        ));
        assert_eq!(pending.state, ActionState::Ready);
        assert_eq!(pending.transfer_retry(ID, T0 + HOUR_NS), Ok(true));
        assert_eq!(
            pending.begin_transfer(T0 + HOUR_NS, no_fresh_time),
            created_at
        );
        // A call that did not run tells nothing of the earlier one.
        assert!(
            end(
                &mut pending,
                Reply::NotRun("busy".to_string()),
                T0 + HOUR_NS
            )
            .is_err()
        );
        assert_eq!(
            pending.begin_transfer(T0 + 2 * HOUR_NS, no_fresh_time),
            created_at
        );
        assert_eq!(
            end(&mut pending, Reply::Sent(Nat::from(3u8)), T0 + 2 * HOUR_NS),
            Ok(Some(Nat::from(3u8)))
        );
    }

    #[test]
    fn a_call_that_did_not_run_leaves_the_transfer_ready_for_a_fresh_one() {
        let mut pending = transfer();
        pending.begin_transfer(T0, || T0);
        assert_eq!(
            end(&mut pending, Reply::NotRun("stopped".to_string()), T0),
            Err(WalletError::ChainError("stopped".to_string()))
        );
        assert_eq!(pending.state, ActionState::Ready);
        assert_eq!(pending.transfer_retry(ID, T0 + 1), Ok(false));
        assert_eq!(pending.begin_transfer(T0 + 1, || T0 + 7), T0 + 7);
    }

    #[test]
    fn a_refusal_fails_the_transfer() {
        let mut pending = transfer();
        pending.begin_transfer(T0, || T0);
        assert_eq!(
            end(&mut pending, Reply::Refused("poor".to_string()), T0),
            Ok(None)
        );
        assert_eq!(pending.state, ActionState::Failed("poor".to_string()));
    }

    #[test]
    fn after_an_unknown_outcome_a_refusal_leaves_the_transfer_open() {
        let mut pending = transfer();
        let created_at = pending.begin_transfer(T0, || T0);
        assert!(end(&mut pending, unknown(), T0).is_err());
        assert_eq!(pending.begin_transfer(T0 + 1, no_fresh_time), created_at);
        // A ledger checks the fee before it looks for the earlier transfer.
        let refused = Reply::Refused("the ledger's fee is 20".to_string());
        assert!(matches!(
            end(&mut pending, refused, T0 + 1),
            Err(WalletError::ChainError(reason)) if reason.contains("may have been sent")
        ));
        assert_eq!(pending.state, ActionState::Ready);
        assert!(pending.may_have_sent());
        assert!(pending.reject(ID, member(1), T0 + 1).is_err());
    }

    #[test]
    fn too_old_fails_only_a_transfer_that_may_have_been_sent() {
        let mut fresh = transfer();
        fresh.begin_transfer(T0, || T0);
        assert!(matches!(
            end(&mut fresh, Reply::TooOld, T0),
            Err(WalletError::ChainError(_))
        ));
        assert_eq!(fresh.state, ActionState::Ready);
        assert_eq!(fresh.transfer, None);

        let mut unseen = transfer();
        unseen.begin_transfer(T0, || T0);
        end(&mut unseen, unknown(), T0).unwrap_err();
        unseen.begin_transfer(T0 + DEDUP_WINDOW_NS, || T0 + DEDUP_WINDOW_NS);
        assert_eq!(
            end(&mut unseen, Reply::TooOld, T0 + DEDUP_WINDOW_NS),
            Ok(None)
        );
        assert!(matches!(
            &unseen.state,
            ActionState::Failed(reason) if reason.contains("cannot tell")
        ));
    }

    #[test]
    fn a_transfer_that_may_have_been_sent_stays_open_and_cannot_be_rejected() {
        let mut pending = transfer();
        let expiry = pending.expires_at_ns;
        let started = expiry - HOUR_NS;
        pending.begin_transfer(started, || started);
        assert!(pending.is_open(expiry));
        end(&mut pending, unknown(), started).unwrap_err();
        assert!(pending.is_open(started + DEDUP_WINDOW_NS - 1));
        assert_eq!(
            pending.view(ID, expiry).expires_at_ns,
            started + DEDUP_WINDOW_NS
        );
        assert!(matches!(
            pending.reject(ID, member(1), expiry),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("may have reached")
        ));
        assert_eq!(
            pending.view(ID, started + DEDUP_WINDOW_NS).state,
            ActionState::Expired
        );

        let mut trapped = transfer();
        trapped.begin_transfer(T0, || T0);
        trapped.lose_transfer_call();
        assert!(trapped.may_have_sent());
        assert_eq!(trapped.transfer_retry(ID, T0), Ok(true));

        let mut untouched = transfer();
        assert_eq!(untouched.reject(ID, member(1), T0), Ok(()));
    }

    /// What the ledger does with a call, and what the wallet hears of it.
    #[derive(Clone, Copy, Debug)]
    enum Ledger {
        Sends,
        SendsUnheard,
        Loses,
        Busy,
        /// Refuses after looking for an earlier transfer, as with too little
        /// in the account.
        Refuses,
        /// Refuses before looking for an earlier transfer, as with a changed
        /// fee.
        RefusesFirst,
        /// Sends, and the wallet's code after the call traps.
        SendsAndTraps,
    }

    #[derive(Clone, Debug)]
    enum Step {
        Execute(Ledger),
        /// Starts a call that never returns to the wallet's code.
        Stall(bool),
        Reject,
        Wait(u64),
    }

    fn step() -> impl Strategy<Value = Step> {
        let ledger = prop_oneof![
            Just(Ledger::Sends),
            Just(Ledger::SendsUnheard),
            Just(Ledger::Loses),
            Just(Ledger::Busy),
            Just(Ledger::Refuses),
            Just(Ledger::RefusesFirst),
            Just(Ledger::SendsAndTraps),
        ];
        prop_oneof![
            3 => ledger.prop_map(Step::Execute),
            1 => any::<bool>().prop_map(Step::Stall),
            1 => Just(Step::Reject),
            3 => prop_oneof![0..=2 * HOUR_NS, 0..=2 * DAY_NS, 0..=8 * DAY_NS].prop_map(Step::Wait),
        ]
    }

    proptest! {
        // The cases that matter need a call near the end of the window.
        #![proptest_config(ProptestConfig::with_cases(2048))]

        /// However the ledger answers and whatever the wallet never hears,
        /// the ledger sends the tokens at most once: once they may have been
        /// sent, every call reuses their `created_at_time`. A transfer ends
        /// `Executed` only if they were sent, and one that was sent unheard
        /// stays open until the ledger no longer finds it.
        #[test]
        fn a_transfer_is_sent_at_most_once(
            start in 0..EXECUTION_WINDOW_NS,
            steps in prop::collection::vec(step(), 1..40),
        ) {
            let mut pending = transfer();
            let mut now = T0 + start;
            // The `created_at_time` of the transfer the ledger sent.
            let mut sent: Option<u64> = None;
            for step in steps {
                let call = |pending: &mut StoredPending, sends: bool| -> Option<(u64, bool)> {
                    if pending.check_executable(ID, now).is_err() || pending.transfer_retry(ID, now).is_err() {
                        return None;
                    }
                    let created_at = pending.begin_transfer(now, || now);
                    let too_old = now >= created_at + DEDUP_WINDOW_NS;
                    let found = sent == Some(created_at) && !too_old;
                    Some((created_at, sends && !too_old && !found))
                };
                match step {
                    Step::Wait(ns) => now += ns,
                    Step::Reject => {
                        let _ = pending.reject(ID, member(1), now);
                    }
                    Step::Stall(sends) => {
                        if let Some((created_at, runs)) = call(&mut pending, sends) {
                            if let Some(earlier) = sent {
                                prop_assert_eq!(created_at, earlier);
                            }
                            if runs {
                                sent = Some(created_at);
                            }
                        }
                    }
                    Step::Execute(ledger) => {
                        let sends = matches!(ledger, Ledger::Sends | Ledger::SendsUnheard | Ledger::SendsAndTraps);
                        let Some((created_at, runs)) = call(&mut pending, sends) else {
                            continue;
                        };
                        if let Some(earlier) = sent {
                            prop_assert_eq!(created_at, earlier, "a second transfer");
                        }
                        let reply = if now >= created_at + DEDUP_WINDOW_NS {
                            Reply::TooOld
                        } else if matches!(ledger, Ledger::RefusesFirst) {
                            Reply::Refused("fee".to_string())
                        } else if sent == Some(created_at) {
                            Reply::Sent(Nat::from(0u8))
                        } else {
                            match ledger {
                                Ledger::Sends => Reply::Sent(Nat::from(0u8)),
                                Ledger::SendsUnheard | Ledger::Loses => unknown(),
                                Ledger::Busy => Reply::NotRun("busy".to_string()),
                                Ledger::Refuses => Reply::Refused("poor".to_string()),
                                Ledger::RefusesFirst => unreachable!(),
                                Ledger::SendsAndTraps => unknown(),
                            }
                        };
                        if runs {
                            sent = Some(created_at);
                        }
                        if matches!(ledger, Ledger::SendsAndTraps) && runs {
                            pending.lose_transfer_call();
                        } else {
                            let _ = end(&mut pending, reply, now);
                        }
                    }
                }
                pending.expire(now);
                if matches!(pending.state, ActionState::Executed(_)) {
                    prop_assert!(sent.is_some());
                }
                if let Some(created_at) = sent
                    && !pending.is_open(now)
                    && !matches!(pending.state, ActionState::Executed(_))
                {
                    prop_assert!(now >= created_at + DEDUP_WINDOW_NS, "{:?}", pending.state);
                }
            }
        }
    }
}
