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
//!
//! A Bitcoin transfer builds its transaction once, and every later step signs
//! or sends that same transaction, so the network can confirm it at most
//! once. Once a send may have reached the Bitcoin canister, the transfer
//! stays open until an execution sees the Bitcoin canister take it, or the
//! wallet sees the network confirm it. Either way the wallet keeps watching
//! the transaction until it settles (see `btc::Watched`).
//!
//! An upgrade whose install was sent stays open until the new version or
//! the install's deadline ends it, and cannot be rejected meanwhile.

use crate::calls::CallFailure;
use crate::icrc::{DEDUP_WINDOW_NS, Reply, STALE_CALL_NS};
use crate::policy::{DAY_NS, HOUR_NS, Terms};
use crate::{
    AccountId, Action, ActionId, ActionState, Chain, Outcome, Pending, RequestId, TxReceipt,
    WalletError,
};
use crate::{btc, evm};
use candid::{CandidType, Nat, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

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
/// At most this many of the open proposals are one proposer's, so an agent
/// cannot crowd the owners out.
pub const MAX_OPEN_BY_PROPOSER: usize = 8;

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
    /// A Bitcoin transfer's transaction and its executions, once `execute`
    /// started one. Absent from actions stored before step 9.
    pub bitcoin: Option<Attempt<BitcoinTx>>,
    /// Whether an upgrade's install was sent. Absent from actions stored
    /// before step 11.
    pub upgrade_sent: Option<bool>,
    /// An EVM transfer's transaction and its executions, once `execute`
    /// started one. Absent from actions stored before step 17.
    pub evm: Option<Attempt<EvmTx>>,
    /// The owner's request a transfer answers (`propose_for`, decision 0012).
    pub request: Option<RequestId>,
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

/// What the wallet knows of a transfer that sends one transaction, built
/// once: a Bitcoin or an EVM transfer.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Attempt<T> {
    /// The executions started so far; the latest owns the transfer.
    pub executions: u64,
    /// When the execution in flight last went on after a call; `None` once
    /// it returned.
    pub in_flight_since_ns: Option<u64>,
    /// Whether the execution in flight is sending the transaction.
    pub sending: bool,
    /// The transaction, once built.
    pub tx: Option<T>,
}

/// A transaction a transfer builds and signs. Once a send of it may have
/// happened, it is only ever sent again as it is, so the chain runs it at
/// most once.
pub trait Tx: Clone {
    /// Who a send hands it to.
    const SENT_TO: &'static str;
    /// Whether the next execution builds it again, with the fees of then,
    /// while no send of it can have reached `SENT_TO`. A Bitcoin
    /// transaction keeps the inputs it reserved instead.
    const REBUILT_UNSENT: bool;
    fn attempt(pending: &StoredPending) -> Option<&Attempt<Self>>;
    fn attempt_mut(pending: &mut StoredPending) -> &mut Option<Attempt<Self>>;
    /// Whether a send of it may have reached `SENT_TO`.
    fn sent(&self) -> bool;
    fn mark_sent(&mut self);
    /// Counts a send of it at `now`.
    fn count_send(&mut self, now: u64);
    fn set_signed(&mut self, signed: Vec<u8>);
    fn signed(&self) -> Option<Vec<u8>>;
    /// The most it pays in fees, which counts toward the daily limit.
    fn fee(&self) -> Nat;
    /// Whether the wallet no longer watches it: the chain ran it for good,
    /// or for good ran another in its place.
    fn is_settled(&self) -> bool;
    fn mark_settled(&mut self);
}

/// A Bitcoin transfer's transaction. It is built once, so every signature
/// and every send is of this one.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BitcoinTx {
    /// Consensus-encoded, without signatures.
    pub unsigned: ByteBuf,
    /// The value of each output it spends, in input order: each signature
    /// covers it.
    pub values: Vec<u64>,
    /// In satoshis.
    pub fee: u64,
    /// Consensus-encoded with its signatures, once each one verified.
    pub signed: Option<ByteBuf>,
    /// Whether a send of it may have reached the Bitcoin canister.
    pub sent: bool,
    /// How many times it was handed to the Bitcoin canister. Absent from
    /// transactions stored before the wallet sent them again on its own.
    pub sends: Option<u32>,
    pub last_sent_ns: Option<u64>,
    /// The tip height when the wallet first saw its inputs spent.
    pub spent_at_height: Option<u32>,
    /// Whether its inputs stayed spent for `btc::SETTLE_CONFIRMATIONS`
    /// blocks, which ends the watch and releases them.
    pub settled: Option<bool>,
}

impl BitcoinTx {
    /// A transaction just built: unsigned, and never sent.
    pub fn new(unsigned: Vec<u8>, values: Vec<u64>, fee: u64) -> Self {
        BitcoinTx {
            unsigned: ByteBuf::from(unsigned),
            values,
            fee,
            signed: None,
            sent: false,
            sends: None,
            last_sent_ns: None,
            spent_at_height: None,
            settled: None,
        }
    }

    /// The watch over this transaction, `txid`, which spends `inputs` and
    /// pays `change` back to its account.
    pub fn watched(&self, txid: [u8; 32], inputs: Vec<[u8; 36]>, change: u64) -> btc::Watched {
        btc::Watched {
            txid,
            inputs,
            spends: self.values.iter().sum(),
            change,
            sends: self.sends.unwrap_or(1),
            last_sent_ns: self.last_sent_ns.unwrap_or(0),
            spent_at_height: self.spent_at_height,
        }
    }
}

impl Tx for BitcoinTx {
    const SENT_TO: &'static str = "the Bitcoin canister";
    const REBUILT_UNSENT: bool = false;

    fn attempt(pending: &StoredPending) -> Option<&Attempt<Self>> {
        pending.bitcoin.as_ref()
    }

    fn attempt_mut(pending: &mut StoredPending) -> &mut Option<Attempt<Self>> {
        &mut pending.bitcoin
    }

    fn sent(&self) -> bool {
        self.sent
    }

    fn mark_sent(&mut self) {
        self.sent = true;
    }

    fn count_send(&mut self, now: u64) {
        self.sends = Some(self.sends.unwrap_or(0).saturating_add(1));
        self.last_sent_ns = Some(now);
    }

    fn set_signed(&mut self, signed: Vec<u8>) {
        self.signed = Some(ByteBuf::from(signed));
    }

    fn signed(&self) -> Option<Vec<u8>> {
        self.signed.clone().map(ByteBuf::into_vec)
    }

    fn fee(&self) -> Nat {
        Nat::from(self.fee)
    }

    fn is_settled(&self) -> bool {
        self.settled == Some(true)
    }

    fn mark_settled(&mut self) {
        self.settled = Some(true);
    }
}

/// An EVM transfer's transaction: one EIP-1559 transfer of a chain's native
/// token, built with its nonce. The chain runs one transaction of an account
/// per nonce, so it runs this transfer at most once.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EvmTx {
    pub chain_id: u64,
    pub nonce: u64,
    /// The recipient's 20 bytes.
    pub to: ByteBuf,
    /// In wei.
    pub value: Nat,
    pub gas_limit: u64,
    pub max_fee_per_gas: Nat,
    pub max_priority_fee_per_gas: Nat,
    /// The most it pays beyond its gas, in wei: on an OP Stack chain, twice
    /// what the fee oracle named for its L1 data and its operator when it
    /// was built.
    pub beyond_gas: Nat,
    /// With its signature, once it verified.
    pub signed: Option<ByteBuf>,
    /// Whether a send of it may have reached a provider.
    pub sent: bool,
    /// How many times it was handed to the providers.
    pub sends: u32,
    pub last_sent_ns: u64,
    /// How many times the wallet read how the chain stands with it, and
    /// when it last did: each look comes later than the one before.
    pub looks: u32,
    pub last_look_ns: u64,
    /// Whether the wallet read its receipt from a finalized block.
    pub settled: bool,
}

impl EvmTx {
    /// A transaction just built, which pays at most `beyond_gas` beyond
    /// its gas: unsigned, and never sent.
    pub fn new(tx: &evm::Transaction, beyond_gas: Nat) -> Self {
        EvmTx {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            to: ByteBuf::from(tx.to.to_vec()),
            value: tx.value.clone(),
            gas_limit: tx.gas_limit,
            max_fee_per_gas: tx.max_fee_per_gas.clone(),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.clone(),
            beyond_gas,
            signed: None,
            sent: false,
            sends: 0,
            last_sent_ns: 0,
            looks: 0,
            last_look_ns: 0,
            settled: false,
        }
    }

    /// Whether the wallet looks at how the chain stands with it at `now`.
    pub fn look_due(&self, now: u64) -> bool {
        now >= self
            .last_look_ns
            .saturating_add(evm::resend_wait(self.looks))
    }

    /// The transaction it signs and sends.
    pub fn transaction(&self) -> evm::Transaction {
        let mut to = [0; 20];
        to.copy_from_slice(&self.to);
        evm::Transaction {
            chain_id: self.chain_id,
            nonce: self.nonce,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas.clone(),
            max_fee_per_gas: self.max_fee_per_gas.clone(),
            gas_limit: self.gas_limit,
            to,
            value: self.value.clone(),
        }
    }

    /// The most it pays in fees, in wei.
    pub fn max_fee(&self) -> Nat {
        self.transaction().max_fee() + self.beyond_gas.clone()
    }
}

impl Tx for EvmTx {
    const SENT_TO: &'static str = "an EVM provider";
    const REBUILT_UNSENT: bool = true;

    fn attempt(pending: &StoredPending) -> Option<&Attempt<Self>> {
        pending.evm.as_ref()
    }

    fn attempt_mut(pending: &mut StoredPending) -> &mut Option<Attempt<Self>> {
        &mut pending.evm
    }

    fn sent(&self) -> bool {
        self.sent
    }

    fn mark_sent(&mut self) {
        self.sent = true;
    }

    fn count_send(&mut self, now: u64) {
        self.sends = self.sends.saturating_add(1);
        self.last_sent_ns = now;
    }

    fn set_signed(&mut self, signed: Vec<u8>) {
        self.signed = Some(ByteBuf::from(signed));
    }

    fn signed(&self) -> Option<Vec<u8>> {
        self.signed.clone().map(ByteBuf::into_vec)
    }

    fn fee(&self) -> Nat {
        self.max_fee()
    }

    fn is_settled(&self) -> bool {
        self.settled
    }

    fn mark_settled(&mut self) {
        self.settled = true;
    }
}

impl<T: Tx> Attempt<T> {
    /// Whether a send of its transaction may have happened.
    fn may_have_sent(&self) -> bool {
        self.sending || self.tx.as_ref().is_some_and(T::sent)
    }

    /// The execution in flight will not go on; a send it started may have
    /// reached `T::SENT_TO`.
    fn lose(&mut self) {
        if self.sending
            && let Some(tx) = &mut self.tx
        {
            tx.mark_sent();
        }
        self.sending = false;
        self.in_flight_since_ns = None;
    }
}

/// How a send of a transaction ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendReply {
    /// It was taken: the chain may run it.
    Taken,
    /// Nobody can have seen it.
    NotRun(String),
    /// It may have been taken.
    Unknown(String),
}

impl From<Result<(), CallFailure>> for SendReply {
    fn from(result: Result<(), CallFailure>) -> Self {
        match result {
            Ok(()) => SendReply::Taken,
            Err(CallFailure::NotRun(reason)) => SendReply::NotRun(reason),
            Err(CallFailure::Unknown(reason)) => SendReply::Unknown(reason),
        }
    }
}

impl StoredPending {
    /// A new action. `approvals` holds the proposer for a proposal by a
    /// member who approves, and nobody for a proposer's proposal or a join
    /// request, whose proposer is not a member yet.
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
            bitcoin: None,
            upgrade_sent: None,
            evm: None,
            request: None,
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
    /// of the window in which the ledger finds it, if that is later. A
    /// Bitcoin or EVM transfer stays open while an execution is in flight,
    /// and for good once its transaction may have been sent.
    fn open_until(&self) -> u64 {
        if self.tx_may_have_sent() || self.is_installing() {
            return u64::MAX;
        }
        let ledger = match &self.transfer {
            Some(attempt) if attempt.unknown || attempt.in_flight_since_ns.is_some() => self
                .expires_at_ns
                .max(attempt.created_at_ns.saturating_add(DEDUP_WINDOW_NS)),
            _ => self.expires_at_ns,
        };
        let in_flight =
            |since: Option<u64>| since.map_or(0, |since| since.saturating_add(STALE_CALL_NS));
        let bitcoin = in_flight(
            self.bitcoin
                .as_ref()
                .and_then(|attempt| attempt.in_flight_since_ns),
        );
        let evm = in_flight(
            self.evm
                .as_ref()
                .and_then(|attempt| attempt.in_flight_since_ns),
        );
        ledger.max(bitcoin).max(evm)
    }

    /// Whether a ledger call of this transfer may have run unseen, or its
    /// transaction may have been sent.
    pub fn may_have_sent(&self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|attempt| attempt.unknown || attempt.in_flight_since_ns.is_some())
            || self.tx_may_have_sent()
    }

    /// Whether the wallet still settles this action on its own: a ledger
    /// call that may have run unseen, which it retries while the ledger can
    /// find it, or a transaction that may have been sent and has not
    /// settled, which it watches.
    pub fn needs_settling(&self) -> bool {
        let open = matches!(self.state, ActionState::Waiting | ActionState::Ready);
        let ledger = open
            && self
                .transfer
                .as_ref()
                .is_some_and(|attempt| attempt.unknown || attempt.in_flight_since_ns.is_some());
        ledger || self.watches::<BitcoinTx>() || self.watches::<EvmTx>()
    }

    /// Whether this transfer's transaction of type `T` may have been sent
    /// and has not settled.
    pub fn watches<T: Tx>(&self) -> bool {
        T::attempt(self).is_some_and(|attempt| {
            attempt.may_have_sent() && attempt.tx.as_ref().is_some_and(|tx| !tx.is_settled())
        })
    }

    fn tx_may_have_sent(&self) -> bool {
        self.bitcoin.as_ref().is_some_and(Attempt::may_have_sent)
            || self.evm.as_ref().is_some_and(Attempt::may_have_sent)
    }

    /// Records that the chain ran this transfer's transaction for good: a
    /// transfer still open ends `Executed`, and any execution in flight
    /// stops. Returns whether it ended now, when its transfer is to be
    /// recorded.
    pub fn settled<T: Tx>(
        &mut self,
        account: AccountId,
        chain: Chain,
        tx_id: String,
        now: u64,
    ) -> bool {
        let open = matches!(self.state, ActionState::Waiting | ActionState::Ready);
        let Some(attempt) = T::attempt_mut(self) else {
            return false;
        };
        let Some(tx) = &mut attempt.tx else {
            return false;
        };
        tx.mark_sent();
        tx.mark_settled();
        if !open {
            return false;
        }
        attempt.executions += 1;
        attempt.in_flight_since_ns = None;
        attempt.sending = false;
        self.state = ActionState::Executed(Outcome::Sent(TxReceipt {
            account,
            chain,
            tx_id,
            at_ns: now,
        }));
        true
    }

    /// Records that the chain settled this transfer without running its
    /// transaction as sent, for `reason`: it reverted. The transfer ends
    /// `Failed`, even one recorded `Executed` when a provider took it.
    /// Returns whether it was recorded so.
    pub fn settled_failed<T: Tx>(&mut self, reason: String) -> bool {
        let executed = matches!(self.state, ActionState::Executed(_));
        let Some(attempt) = T::attempt_mut(self) else {
            return false;
        };
        let Some(tx) = &mut attempt.tx else {
            return false;
        };
        tx.mark_sent();
        tx.mark_settled();
        attempt.executions += 1;
        attempt.in_flight_since_ns = None;
        attempt.sending = false;
        if executed || matches!(self.state, ActionState::Waiting | ActionState::Ready) {
            self.state = ActionState::Failed(reason);
        }
        executed
    }

    /// Stores what the wallet saw of this transfer's transaction on the
    /// network.
    pub fn bitcoin_observed(&mut self, watched: &btc::Watched) {
        if let Some(tx) = self
            .bitcoin
            .as_mut()
            .and_then(|attempt| attempt.tx.as_mut())
        {
            tx.spent_at_height = watched.spent_at_height;
        }
    }

    /// Counts a look at how the chain stands with this transfer's EVM
    /// transaction, at `now`.
    pub fn evm_looked(&mut self, now: u64) {
        if let Some(tx) = self.evm.as_mut().and_then(|attempt| attempt.tx.as_mut()) {
            tx.looks = tx.looks.saturating_add(1);
            tx.last_look_ns = now;
        }
    }

    /// Counts a send of this executed transfer's transaction at `now`.
    pub fn resent<T: Tx>(&mut self, now: u64) {
        if let Some(tx) = T::attempt_mut(self)
            .as_mut()
            .and_then(|attempt| attempt.tx.as_mut())
        {
            tx.count_send(now);
        }
    }

    /// Whether this upgrade's install was sent and has not ended.
    pub fn is_installing(&self) -> bool {
        self.upgrade_sent == Some(true)
            && matches!(self.state, ActionState::Waiting | ActionState::Ready)
    }

    /// Records that this upgrade's install was sent.
    pub fn install_sent(&mut self) {
        self.upgrade_sent = Some(true);
    }

    /// The transaction of type `T`, once built.
    pub fn tx<T: Tx>(&self) -> Option<&T> {
        T::attempt(self).and_then(|attempt| attempt.tx.as_ref())
    }

    /// The Bitcoin transaction, once built.
    pub fn bitcoin_tx(&self) -> Option<&BitcoinTx> {
        self.tx()
    }

    /// The EVM transaction, once built.
    pub fn evm_tx(&self) -> Option<&EvmTx> {
        self.tx()
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
        if self.is_installing() {
            return Err(WalletError::UpgradeInProgress);
        }
        if self.may_have_sent() {
            let reason = if self.bitcoin.as_ref().is_some_and(Attempt::may_have_sent) {
                "may have reached the Bitcoin canister; execute it again to send the same transaction"
            } else if self.evm.as_ref().is_some_and(Attempt::may_have_sent) {
                "may have reached an EVM provider; execute it again to send the same transaction"
            } else {
                "may have reached the ledger; execute it again to find out"
            };
            return Err(WalletError::InvalidArgument(format!(
                "action {id} {reason}"
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

    /// Records that execution `execution` of a transfer that sends a
    /// transaction will not go on: the callback of one of its calls
    /// trapped. A transaction it was sending may have been sent.
    pub fn lose_send_call<T: Tx>(&mut self, execution: u64) {
        if self.owns_send::<T>(execution)
            && let Some(attempt) = T::attempt_mut(self)
        {
            attempt.lose();
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

    /// Starts an execution of this transfer, which sends a transaction of
    /// type `T`, and returns its number. Refuses while another is in flight.
    /// One still marked in flight after `STALE_CALL_NS` never returned to
    /// the wallet's code, since each of its calls answers within 300
    /// seconds.
    pub fn begin_send<T: Tx>(&mut self, id: ActionId, now: u64) -> Result<u64, WalletError> {
        let attempt = T::attempt_mut(self).get_or_insert(Attempt {
            executions: 0,
            in_flight_since_ns: None,
            sending: false,
            tx: None,
        });
        if let Some(since) = attempt.in_flight_since_ns {
            if now < since.saturating_add(STALE_CALL_NS) {
                return Err(WalletError::InvalidArgument(format!(
                    "an execution of action {id} is in flight"
                )));
            }
            attempt.lose();
        }
        attempt.executions += 1;
        attempt.in_flight_since_ns = Some(now);
        Ok(attempt.executions)
    }

    /// Whether an execution of this transfer, which sends a transaction of
    /// type `T`, is in flight at `now`: one in flight longer than
    /// `STALE_CALL_NS` never returned to the wallet's code.
    pub fn executing<T: Tx>(&self, now: u64) -> bool {
        T::attempt(self)
            .and_then(|attempt| attempt.in_flight_since_ns)
            .is_some_and(|since| now < since.saturating_add(STALE_CALL_NS))
    }

    /// Forgets the transaction of type `T` when no send of it can have
    /// reached `T::SENT_TO`, and returns whether it did.
    pub fn unbuild_unsent<T: Tx>(&mut self) -> bool {
        match T::attempt_mut(self) {
            Some(attempt) if attempt.tx.is_some() && !attempt.may_have_sent() => {
                attempt.tx = None;
                true
            }
            _ => false,
        }
    }

    /// Whether execution `execution` is the one in flight.
    pub fn owns_send<T: Tx>(&self, execution: u64) -> bool {
        T::attempt(self).is_some_and(|attempt| {
            attempt.executions == execution && attempt.in_flight_since_ns.is_some()
        })
    }

    /// Goes on with execution `execution` after a call, at `now`: refuses
    /// when the action closed meanwhile or a later execution took over.
    pub fn resume_send<T: Tx>(
        &mut self,
        id: ActionId,
        execution: u64,
        now: u64,
    ) -> Result<(), WalletError> {
        self.check_open(id, now)?;
        if !self.owns_send::<T>(execution) {
            return Err(WalletError::InvalidArgument(format!(
                "another execution of action {id} took over"
            )));
        }
        if let Some(attempt) = T::attempt_mut(self) {
            attempt.in_flight_since_ns = Some(now);
        }
        Ok(())
    }

    /// Stores the transaction the execution in flight built.
    pub fn built<T: Tx>(&mut self, tx: T) {
        if let Some(attempt) = T::attempt_mut(self) {
            attempt.tx = Some(tx);
        }
    }

    /// Stores the signed transaction.
    pub fn signed<T: Tx>(&mut self, signed: Vec<u8>) {
        if let Some(tx) = T::attempt_mut(self)
            .as_mut()
            .and_then(|attempt| attempt.tx.as_mut())
        {
            tx.set_signed(signed);
        }
    }

    /// Marks the execution in flight as sending the transaction at `now`.
    pub fn sending<T: Tx>(&mut self, now: u64) {
        if let Some(attempt) = T::attempt_mut(self) {
            attempt.sending = true;
            if let Some(tx) = &mut attempt.tx {
                tx.count_send(now);
            }
        }
    }

    /// Records that the execution in flight returned without sending.
    pub fn end_send_call<T: Tx>(&mut self) {
        if let Some(attempt) = T::attempt_mut(self) {
            attempt.in_flight_since_ns = None;
        }
    }

    /// Records how sending the transaction `sending` started ended, from
    /// `account` on `chain`. A transaction taken ends the action
    /// `Executed`; otherwise it stays ready, and the error says why.
    pub fn end_send<T: Tx>(
        &mut self,
        reply: SendReply,
        account: AccountId,
        chain: Chain,
        tx_id: String,
        now: u64,
    ) -> Result<(), WalletError> {
        let Some(attempt) = T::attempt_mut(self) else {
            return Err(WalletError::InvalidArgument(
                "no transaction was sent".to_string(),
            ));
        };
        attempt.in_flight_since_ns = None;
        attempt.sending = false;
        let sent_before = attempt.tx.as_ref().is_some_and(T::sent);
        let to = T::SENT_TO;
        let error = match reply {
            SendReply::Taken => {
                if let Some(tx) = &mut attempt.tx {
                    tx.mark_sent();
                }
                self.state = ActionState::Executed(Outcome::Sent(TxReceipt {
                    account,
                    chain,
                    tx_id,
                    at_ns: now,
                }));
                return Ok(());
            }
            SendReply::NotRun(reason) if sent_before => {
                format!("{reason}; an earlier send may have reached {to}, so execute it again")
            }
            SendReply::NotRun(reason) => reason,
            SendReply::Unknown(reason) => {
                if let Some(tx) = &mut attempt.tx {
                    tx.mark_sent();
                }
                format!(
                    "{reason}; {to} may have taken the transaction, so execute it again to send \
                     the same one"
                )
            }
        };
        Err(WalletError::ChainError(error))
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
            request: current.request,
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
    fn a_sent_upgrade_stays_open_and_cannot_be_rejected_until_it_ends() {
        let upgrade = Action::Upgrade {
            version: "1.0.0".to_string(),
            wasm_sha256: ByteBuf::from(vec![0; 32]),
        };
        let terms = Terms {
            quorum: 1,
            timelock_ns: 0,
            daily_limit: false,
        };
        let mut pending = StoredPending::new(
            upgrade,
            member(1),
            vec![member(1)],
            terms,
            APPROVAL_WINDOW_NS,
            T0,
        );
        let expiry = pending.expires_at_ns;
        assert!(!pending.is_installing());
        pending.install_sent();
        assert!(pending.is_installing());
        assert!(pending.is_open(expiry + DAY_NS));
        assert_eq!(
            pending.clone().reject(ID, member(1), expiry + DAY_NS),
            Err(WalletError::UpgradeInProgress)
        );
        pending.finish(Err("trapped".to_string()));
        assert!(!pending.is_installing());
        assert!(!pending.is_open(T0));
        assert!(pending.reject(ID, member(1), T0).is_err());
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
                asset: Some(Chain::Icp),
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

    fn built(fee: u64) -> BitcoinTx {
        BitcoinTx::new(vec![1, 2, 3], vec![10_000], fee)
    }

    fn end_send(
        pending: &mut StoredPending,
        result: Result<(), CallFailure>,
    ) -> Result<(), WalletError> {
        let chain = Chain::Bitcoin(crate::BitcoinNetwork::Regtest);
        pending.end_send::<BitcoinTx>(result.into(), 1, chain, "txid".to_string(), T0)
    }

    #[test]
    fn one_bitcoin_execution_at_a_time_and_a_stale_one_is_taken_over() {
        let mut pending = transfer();
        let first = pending.begin_send::<BitcoinTx>(ID, T0).unwrap();
        assert!(matches!(
            pending.begin_send::<BitcoinTx>(ID, T0 + STALE_CALL_NS - 1),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("in flight")
        ));
        // Each step after a call keeps the execution alive.
        assert_eq!(
            pending.resume_send::<BitcoinTx>(ID, first, T0 + HOUR_NS - 1),
            Ok(())
        );
        assert!(pending.begin_send::<BitcoinTx>(ID, T0 + HOUR_NS).is_err());

        let second = pending
            .begin_send::<BitcoinTx>(ID, T0 + 2 * HOUR_NS)
            .unwrap();
        assert_ne!(first, second);
        assert!(matches!(
            pending.resume_send::<BitcoinTx>(ID, first, T0 + 2 * HOUR_NS),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("took over")
        ));
        // A trap in the first execution's callback changes nothing now.
        pending.sending::<BitcoinTx>(T0);
        let before = pending.clone();
        pending.lose_send_call::<BitcoinTx>(first);
        assert_eq!(pending, before);
        pending.lose_send_call::<BitcoinTx>(second);
        assert!(!pending.owns_send::<BitcoinTx>(second));
    }

    #[test]
    fn every_send_is_counted_and_the_transaction_is_watched_until_it_settles() {
        let mut pending = transfer();
        assert!(!pending.needs_settling());
        let execution = pending.begin_send::<BitcoinTx>(ID, T0).unwrap();
        pending.built(built(500));
        pending.signed::<BitcoinTx>(vec![9]);
        assert!(!pending.needs_settling(), "built and signed, not sent");
        pending.sending::<BitcoinTx>(T0 + 1);
        assert!(pending.needs_settling());
        assert_eq!(end_send(&mut pending, Ok(())), Ok(()));
        assert!(!pending.is_open(T0));
        assert!(!pending.owns_send::<BitcoinTx>(execution));
        let tx = pending.bitcoin_tx().unwrap();
        assert_eq!((tx.sends, tx.last_sent_ns), (Some(1), Some(T0 + 1)));
        assert!(pending.needs_settling(), "executed, not yet settled");

        pending.resent::<BitcoinTx>(T0 + HOUR_NS);
        let tx = pending.bitcoin_tx().unwrap();
        assert_eq!((tx.sends, tx.last_sent_ns), (Some(2), Some(T0 + HOUR_NS)));
        let watched = tx.watched([2; 32], vec![[1; 36]], 7);
        assert_eq!(
            (watched.sends, watched.last_sent_ns, watched.change),
            (2, T0 + HOUR_NS, 7)
        );

        let chain = Chain::Bitcoin(crate::BitcoinNetwork::Regtest);
        let receipt = pending.state.clone();
        assert!(!pending.settled::<BitcoinTx>(1, chain, "txid".to_string(), T0 + DAY_NS));
        assert_eq!(pending.state, receipt, "the receipt of its send stays");
        assert!(!pending.needs_settling());
    }

    #[test]
    fn a_transaction_seen_confirmed_ends_its_open_transfer_and_stops_its_execution() {
        let mut pending = transfer();
        let execution = pending.begin_send::<BitcoinTx>(ID, T0).unwrap();
        pending.built(built(500));
        pending.signed::<BitcoinTx>(vec![9]);
        pending.sending::<BitcoinTx>(T0);
        // The send's answer was lost: the transfer stays open.
        assert!(
            end_send(
                &mut pending,
                Err(CallFailure::Unknown("timeout".to_string()))
            )
            .is_err()
        );
        assert!(pending.is_open(T0 + 30 * DAY_NS));
        assert!(pending.needs_settling());
        let again = pending.begin_send::<BitcoinTx>(ID, T0 + 1).unwrap();
        assert_ne!(execution, again);

        let chain = Chain::Bitcoin(crate::BitcoinNetwork::Regtest);
        assert!(pending.settled::<BitcoinTx>(1, chain.clone(), "txid".to_string(), T0 + 2));
        assert_eq!(
            pending.state,
            ActionState::Executed(Outcome::Sent(TxReceipt {
                account: 1,
                chain,
                tx_id: "txid".to_string(),
                at_ns: T0 + 2,
            }))
        );
        assert!(
            !pending.owns_send::<BitcoinTx>(again),
            "the execution in flight stops"
        );
        assert!(!pending.needs_settling());
        assert!(pending.bitcoin_tx().unwrap().is_settled());
    }

    #[test]
    fn a_ledger_call_that_may_have_run_needs_settling_while_the_transfer_is_open() {
        let mut pending = transfer();
        pending.begin_transfer(T0, || T0);
        assert!(pending.needs_settling());
        pending.lose_transfer_call();
        assert!(pending.needs_settling());
        assert_eq!(
            pending.end_transfer(Reply::Sent(Nat::from(3u8)), 1, Chain::Icp, T0),
            Ok(Some(Nat::from(3u8)))
        );
        assert!(!pending.needs_settling());
    }

    #[test]
    fn a_built_transaction_is_kept_and_one_not_sent_can_be_rejected() {
        let mut pending = transfer();
        let execution = pending.begin_send::<BitcoinTx>(ID, T0).unwrap();
        pending.built(built(500));
        pending.signed::<BitcoinTx>(vec![9]);
        pending.end_send_call::<BitcoinTx>();
        assert!(!pending.may_have_sent());
        assert_eq!(pending.bitcoin_tx().map(|tx| tx.fee), Some(500));

        // The next execution finds the same transaction.
        let next = pending.begin_send::<BitcoinTx>(ID, T0 + 1).unwrap();
        assert!(next > execution);
        assert_eq!(
            pending.bitcoin_tx().unwrap().signed,
            Some(ByteBuf::from(vec![9]))
        );
        // Signing sends nothing, so it can still be rejected, and the
        // execution in flight stops at its next step.
        assert_eq!(pending.reject(ID, member(1), T0 + 1), Ok(()));
        assert!(pending.resume_send::<BitcoinTx>(ID, next, T0 + 2).is_err());
    }

    fn evm_built() -> EvmTx {
        EvmTx::new(
            &evm::Transaction {
                chain_id: evm::ETHEREUM,
                nonce: 3,
                max_priority_fee_per_gas: Nat::from(1u8),
                max_fee_per_gas: Nat::from(2u8),
                gas_limit: 21_000,
                to: [7; 20],
                value: Nat::from(5u8),
            },
            Nat::from(0u8),
        )
    }

    fn end_evm_send(pending: &mut StoredPending, reply: SendReply) -> Result<(), WalletError> {
        let chain = Chain::Evm(evm::ETHEREUM);
        pending.end_send::<EvmTx>(reply, 1, chain, "hash".to_string(), T0)
    }

    #[test]
    fn an_evm_transaction_never_sent_is_built_again_and_one_maybe_sent_is_kept() {
        let mut pending = transfer();
        pending.begin_send::<EvmTx>(ID, T0).unwrap();
        assert!(pending.executing::<EvmTx>(T0 + 1));
        pending.built(evm_built());
        pending.signed::<EvmTx>(vec![9]);
        pending.sending::<EvmTx>(T0 + 1);
        // The call never ran, so no provider saw it.
        assert!(end_evm_send(&mut pending, SendReply::NotRun("down".to_string())).is_err());
        assert!(!pending.executing::<EvmTx>(T0 + 2));
        assert!(pending.unbuild_unsent::<EvmTx>());
        assert_eq!(pending.evm_tx(), None);
        assert!(!pending.unbuild_unsent::<EvmTx>());

        // One a provider may have taken is kept, to be sent the same.
        pending.begin_send::<EvmTx>(ID, T0 + 3).unwrap();
        pending.built(evm_built());
        pending.signed::<EvmTx>(vec![9]);
        pending.sending::<EvmTx>(T0 + 4);
        assert!(end_evm_send(&mut pending, SendReply::Unknown("lost".to_string())).is_err());
        assert!(!pending.unbuild_unsent::<EvmTx>());
        assert_eq!(
            pending.evm_tx().unwrap().signed,
            Some(ByteBuf::from(vec![9]))
        );

        // An execution in flight for `STALE_CALL_NS` never returned.
        pending.begin_send::<EvmTx>(ID, T0 + 5).unwrap();
        assert!(pending.executing::<EvmTx>(T0 + 5 + STALE_CALL_NS - 1));
        assert!(!pending.executing::<EvmTx>(T0 + 5 + STALE_CALL_NS));
    }

    #[test]
    fn the_wallet_looks_at_an_evm_transaction_less_often_each_time() {
        let mut pending = transfer();
        pending.begin_send::<EvmTx>(ID, T0).unwrap();
        pending.built(evm_built());
        assert!(pending.evm_tx().unwrap().look_due(T0));
        pending.evm_looked(T0);
        let tx = pending.evm_tx().unwrap();
        assert_eq!((tx.looks, tx.last_look_ns), (1, T0));
        let first = evm::FIRST_RESEND_NS;
        assert!(!tx.look_due(T0 + first - 1));
        assert!(tx.look_due(T0 + first));
        pending.evm_looked(T0 + first);
        let tx = pending.evm_tx().unwrap();
        assert!(!tx.look_due(T0 + 3 * first - 1));
        assert!(tx.look_due(T0 + 3 * first));
    }

    #[test]
    fn a_transaction_that_may_have_been_sent_keeps_its_transfer_open() {
        let mut pending = transfer();
        pending.begin_send::<BitcoinTx>(ID, T0).unwrap();
        pending.built(built(500));
        pending.sending::<BitcoinTx>(T0);
        assert!(pending.may_have_sent());
        assert!(pending.reject(ID, member(1), T0).is_err());

        assert!(matches!(
            end_send(&mut pending, Err(CallFailure::Unknown("timeout".to_string()))),
            Err(WalletError::ChainError(reason)) if reason.contains("same one")
        ));
        assert_eq!(pending.state, ActionState::Ready);
        assert!(pending.may_have_sent());
        let never = u64::MAX - 1;
        assert_eq!(pending.view(ID, never).expires_at_ns, u64::MAX);
        assert_eq!(pending.view(ID, never).state, ActionState::Ready);

        // A later send that did not run tells nothing of the earlier one.
        pending.begin_send::<BitcoinTx>(ID, never).unwrap();
        pending.sending::<BitcoinTx>(T0);
        assert!(end_send(&mut pending, Err(CallFailure::NotRun("full".to_string()))).is_err());
        assert!(pending.may_have_sent());

        pending.begin_send::<BitcoinTx>(ID, never).unwrap();
        pending.sending::<BitcoinTx>(T0);
        assert_eq!(end_send(&mut pending, Ok(())), Ok(()));
        assert!(matches!(
            &pending.state,
            ActionState::Executed(Outcome::Sent(receipt)) if receipt.tx_id == "txid"
        ));
    }

    #[test]
    fn a_send_that_did_not_run_and_a_trap_before_sending_leave_it_unsent() {
        let mut pending = transfer();
        let execution = pending.begin_send::<BitcoinTx>(ID, T0).unwrap();
        pending.built(built(500));
        pending.sending::<BitcoinTx>(T0);
        assert_eq!(
            end_send(&mut pending, Err(CallFailure::NotRun("full".to_string()))),
            Err(WalletError::ChainError("full".to_string()))
        );
        assert!(!pending.may_have_sent());
        assert!(!pending.owns_send::<BitcoinTx>(execution));

        // A trap while signing sends nothing; one while sending may have.
        let signing = pending.begin_send::<BitcoinTx>(ID, T0 + 1).unwrap();
        pending.lose_send_call::<BitcoinTx>(signing);
        assert!(!pending.may_have_sent());
        let sending = pending.begin_send::<BitcoinTx>(ID, T0 + 2).unwrap();
        pending.sending::<BitcoinTx>(T0);
        pending.lose_send_call::<BitcoinTx>(sending);
        assert!(pending.may_have_sent());
        assert!(pending.begin_send::<BitcoinTx>(ID, T0 + 3).is_ok());
    }

    /// What the Bitcoin canister does with a send, and what the wallet hears.
    #[derive(Clone, Copy, Debug)]
    enum Send {
        Takes,
        TakesUnheard,
        Loses,
        Full,
        /// Takes it, and the wallet's code after the call traps.
        TakesAndTraps,
        /// Never returns to the wallet's code.
        Stalls,
    }

    #[derive(Clone, Debug)]
    enum BitcoinStep {
        /// An execution: whether the build and the signing succeed, and how
        /// the send ends.
        Execute(bool, bool, Send),
        Reject,
        Wait(u64),
    }

    fn bitcoin_step() -> impl Strategy<Value = BitcoinStep> {
        let send = prop_oneof![
            Just(Send::Takes),
            Just(Send::TakesUnheard),
            Just(Send::Loses),
            Just(Send::Full),
            Just(Send::TakesAndTraps),
            Just(Send::Stalls),
        ];
        prop_oneof![
            4 => (prop::bool::weighted(0.9), prop::bool::weighted(0.8), send)
                .prop_map(|(build, sign, send)| BitcoinStep::Execute(build, sign, send)),
            1 => Just(BitcoinStep::Reject),
            3 => prop_oneof![0..=2 * HOUR_NS, 0..=8 * DAY_NS].prop_map(BitcoinStep::Wait),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        /// Driven as `execute` drives it, a Bitcoin transfer builds one
        /// transaction, so the network is sent at most that one; it ends
        /// `Executed` only once the Bitcoin canister took it; and once it may
        /// have been taken, the transfer stays open and cannot be rejected.
        #[test]
        fn a_bitcoin_transfer_sends_at_most_one_transaction(
            start in 0..EXECUTION_WINDOW_NS,
            steps in prop::collection::vec(bitcoin_step(), 1..40),
        ) {
            let mut pending = transfer();
            let mut now = T0 + start;
            let mut builds = 0u8;
            // The transactions the Bitcoin canister took.
            let mut taken: Vec<ByteBuf> = Vec::new();
            for step in steps {
                match step {
                    BitcoinStep::Wait(ns) => now += ns,
                    BitcoinStep::Reject => {
                        let may_have_sent = pending.may_have_sent();
                        if pending.reject(ID, member(1), now).is_ok() {
                            prop_assert!(!may_have_sent && taken.is_empty());
                        }
                    }
                    BitcoinStep::Execute(builds_ok, signs_ok, send) => {
                        if pending.check_executable(ID, now).is_err() {
                            continue;
                        }
                        let Ok(execution) = pending.begin_send::<BitcoinTx>(ID, now) else {
                            continue;
                        };
                        if pending.bitcoin_tx().is_none() {
                            if !builds_ok {
                                pending.end_send_call::<BitcoinTx>();
                                pending.finish(Err("too little".to_string()));
                                continue;
                            }
                            builds += 1;
                            pending.built(BitcoinTx {
                                unsigned: ByteBuf::from(vec![builds]),
                                ..built(500)
                            });
                        }
                        if pending.bitcoin_tx().unwrap().signed.is_none() {
                            if !signs_ok {
                                pending.end_send_call::<BitcoinTx>();
                                continue;
                            }
                            let unsigned = pending.bitcoin_tx().unwrap().unsigned.to_vec();
                            pending.signed::<BitcoinTx>(unsigned);
                        }
                        let tx = pending.bitcoin_tx().unwrap().signed.clone().unwrap();
                        pending.sending::<BitcoinTx>(T0);
                        if matches!(send, Send::Takes | Send::TakesUnheard | Send::TakesAndTraps) {
                            taken.push(tx);
                        }
                        let chain = Chain::Bitcoin(crate::BitcoinNetwork::Regtest);
                        let result = match send {
                            Send::Takes => Ok(()),
                            Send::TakesUnheard | Send::Loses => Err(CallFailure::Unknown("timeout".to_string())),
                            Send::Full => Err(CallFailure::NotRun("full".to_string())),
                            Send::TakesAndTraps => {
                                pending.lose_send_call::<BitcoinTx>(execution);
                                continue;
                            }
                            Send::Stalls => continue,
                        };
                        let _ = pending.end_send::<BitcoinTx>(result.into(), 1, chain, "txid".to_string(), now);
                    }
                }
                pending.expire(now);
                taken.dedup();
                prop_assert!(taken.len() <= 1, "two transactions: {:?}", taken);
                prop_assert!(builds <= 1, "built {} transactions", builds);
                if matches!(pending.state, ActionState::Executed(_)) {
                    prop_assert!(!taken.is_empty());
                }
                if !taken.is_empty() {
                    prop_assert!(
                        pending.is_open(now) || matches!(pending.state, ActionState::Executed(_)),
                        "{:?}", pending.state
                    );
                }
            }
        }
    }
}
