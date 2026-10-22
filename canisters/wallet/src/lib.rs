//! Firmware of a self-owned chain-key wallet canister.
//!
//! Steps 1 to 11 of `docs/plan-v0.md`: the install arguments, `status`, state
//! in stable memory, the claim, members, the policy with its pending actions,
//! the audit log, the certified pages, ICP and ICRC accounts, Bitcoin
//! accounts signed with threshold ECDSA, with their balances and transfers,
//! and the self-upgrade to a signed release. The interface test lists the
//! methods of `candid/wallet.did` not implemented yet.

mod access;
mod accounts;
mod actions;
mod bitcoin_canister;
mod btc;
mod calls;
mod channel;
mod claim;
mod ed25519;
mod governance;
mod http;
mod ledger;
mod management;
mod policy;
mod signer;
mod storage;
mod transfers;
mod types;
mod upgrade;

pub use access::{NOT_A_MEMBER, NOT_A_PROPOSER, NOT_AN_APPROVER, NOT_AN_OWNER, NOT_CLAIMED};
pub use claim::message as claim_message;
pub use types::*;
pub use upgrade::{FIRST_RESEND_NS, MAX_INSTALL_SENDS, cycles_to_upgrade};

use accounts::StoredAccount;
use actions::{
    APPROVAL_WINDOW_NS, BitcoinTx, JOIN_WINDOW_NS, MAX_OPEN_ACTIONS, MAX_OPEN_BY_PROPOSER,
    MAX_OPEN_JOIN_REQUESTS, StoredPending,
};
use candid::{Nat, Principal};
use policy::{DAY_NS, Spend};
use release::ReleaseManifest;
use serde_bytes::ByteBuf;
use storage::{StoredAuditEntry, StoredMember, StoredTransfer};
use transfers::CallFailure;

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
    http::certify();
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
    http::certify();
    // This version runs, so an install the wallet sent itself ran.
    let now = ic_cdk::api::time();
    let mut upgrade = storage::upgrade();
    if let Some((id, version)) = upgrade.installed() {
        storage::set_upgrade(upgrade);
        end_upgrade(
            id,
            Ok(Outcome::Done),
            AuditEvent::UpgradeInstalled(version),
            now,
        );
    }
    // An upgrade turns the timer off.
    schedule_timer(now);
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
    audit(caller, AuditEvent::Claimed, now);
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

/// The wallet's accounts, in id order.
#[ic_cdk::query(guard = "access::member")]
fn accounts() -> Result<Vec<Account>, WalletError> {
    let wallet = ic_cdk::api::canister_self();
    Ok(storage::accounts()
        .into_iter()
        .map(|(id, account)| account.view(id, wallet))
        .collect())
}

/// Adds an account: the next index on its chain, held by the wallet. Adding
/// one moves nothing, so it needs no approval. A Bitcoin account asks the
/// management canister for the public key of its derivation path first.
#[ic_cdk::update(guard = "access::owner")]
async fn add_account(args: AddAccountArgs) -> Result<Account, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    check_not_installing(ic_cdk::api::time())?;
    let index = accounts::check_new(&args, storage::accounts().iter().map(|(_, a)| a))?;
    let public_key = match args.chain {
        Chain::Bitcoin(network) => {
            let key = signer::public_key(network, index)
                .await
                .map_err(WalletError::ChainError)?;
            btc::address(&key, network).map_err(WalletError::ChainError)?;
            // Other calls ran meanwhile.
            if storage::member(caller).map(|member| member.role) != Some(Role::Owner) {
                return Err(WalletError::Forbidden(
                    "the caller is no longer an owner".to_string(),
                ));
            }
            let again = accounts::check_new(&args, storage::accounts().iter().map(|(_, a)| a))?;
            if again != index {
                return Err(WalletError::InvalidArgument(
                    "another account on this chain was added meanwhile; add this one again"
                        .to_string(),
                ));
            }
            Some(ByteBuf::from(key))
        }
        _ => None,
    };
    let now = ic_cdk::api::time();
    let account = StoredAccount {
        chain: args.chain,
        label: args.label,
        index,
        holder: AccountHolder::Wallet,
        added_at_ns: now,
        added_by: caller,
        public_key,
    };
    let id = storage::add_account(account.clone());
    audit(caller, AuditEvent::AccountAdded(id), now);
    Ok(account.view(id, ic_cdk::api::canister_self()))
}

/// An account's balance, as its ledger or the Bitcoin canister answers it
/// now. A Bitcoin balance counts outputs with at least one confirmation.
#[ic_cdk::update(guard = "access::member")]
async fn balance(id: AccountId) -> Result<Balance, WalletError> {
    check_not_installing(ic_cdk::api::time())?;
    let account =
        storage::account(id).ok_or_else(|| WalletError::NotFound(format!("account {id}")))?;
    let amount = match &account.chain {
        Chain::Bitcoin(network) => {
            let address = account.bitcoin_address()?.to_string();
            bitcoin_canister::balance(*network, address)
                .await
                .map(Nat::from)
        }
        chain => {
            let ledger = accounts::ledger(chain)?;
            ledger::balance_of(ledger, account.ledger_account(ic_cdk::api::canister_self())).await
        }
    }
    .map_err(WalletError::ChainError)?;
    Ok(Balance {
        account: id,
        amount,
        as_of_ns: ic_cdk::api::time(),
    })
}

/// Checks a transfer from `account`, the stored account `request.from`
/// names, and returns what it spends against its chain's daily limit: the
/// amount, and for Bitcoin the fee the proposer fixed.
fn check_transfer(
    request: &TransferRequest,
    account: Option<&StoredAccount>,
) -> Result<Nat, WalletError> {
    match account.map(|account| &account.chain) {
        Some(Chain::Bitcoin(network)) => {
            let checked = btc::check(request, *network)?;
            Ok(Nat::from(checked.amount + checked.fee.unwrap_or(0)))
        }
        _ => transfers::check(request, account).map(|_| request.amount.clone()),
    }
}

/// What an open transfer on `chain` may still spend: the amount, and for
/// Bitcoin the fee of its transaction once built, or the one its proposer
/// fixed.
fn committed(pending: &StoredPending, request: &TransferRequest, chain: &Chain) -> Nat {
    match chain {
        Chain::Bitcoin(_) => {
            let fee = pending
                .bitcoin_tx()
                .map(|tx| Nat::from(tx.fee))
                .or_else(|| request.fee.clone())
                .unwrap_or_default();
            request.amount.clone() + fee
        }
        _ => request.amount.clone(),
    }
}

/// What `chain` sent in the last 24 hours, and what its open transfers under
/// the daily limit may still send, apart from the action `except`.
fn spent(chain: &Chain, now: u64, except: Option<ActionId>) -> Nat {
    let mut spent = storage::sent_since(chain, now.saturating_sub(DAY_NS));
    for (id, pending) in storage::open_actions(now) {
        if Some(id) == except || pending.daily_limit != Some(true) {
            continue;
        }
        if let Action::Transfer(request) = &pending.action
            && storage::account(request.from).is_some_and(|account| &account.chain == chain)
        {
            spent += committed(&pending, request, chain);
        }
    }
    spent
}

/// Checks that action `id`, a transfer the daily limit released, still fits
/// its chain's limit when it spends `amount`.
fn fits_daily_limit(
    id: ActionId,
    chain: &Chain,
    amount: &Nat,
    now: u64,
) -> Result<(), WalletError> {
    let spent = spent(chain, now, Some(id));
    let spend = Spend {
        chain,
        amount,
        spent: &spent,
    };
    let terms = policy::terms(&storage::policy(), ActionKind::Transfer, Some(&spend));
    if terms.quorum > 1 || terms.timelock_ns > 0 {
        return Err(WalletError::PolicyViolation(
            "the transfer no longer fits its chain's daily limit; propose it again".to_string(),
        ));
    }
    Ok(())
}

/// Proposes an action. An owner's approval counts toward its quorum, so
/// with a quorum of one it is ready at once. A transfer that keeps its
/// chain's spending within the daily limit is ready at once too. A proposer
/// proposes only transfers, and its approval never counts: each one waits
/// for an owner or an approver.
#[ic_cdk::update(guard = "access::proposer")]
fn propose(action: Action) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let governance = storage::governance();
    let approves = governance.can_approve(&caller);
    if !approves && !matches!(action, Action::Transfer(_)) {
        return Err(WalletError::Forbidden(
            "a proposer proposes only transfers".to_string(),
        ));
    }
    let spend = match &action {
        Action::Transfer(request) => {
            let account = storage::account(request.from);
            let amount = check_transfer(request, account.as_ref())?;
            account.map(|account| (account.chain, amount))
        }
        Action::Upgrade {
            version,
            wasm_sha256,
        } => {
            upgrade::check_action(version, wasm_sha256)?;
            None
        }
        Action::HandOverControl {
            controller,
            controller_module_hash,
            controller_policy_sha256,
        } => {
            check_hand_over(controller, controller_module_hash, controller_policy_sha256)?;
            None
        }
        _ => {
            governance.check(&action, caller, now)?;
            None
        }
    };
    storage::close_expired(now);
    let open = storage::open_actions(now);
    if open.iter().filter(|(_, p)| !p.is_join_request()).count() >= MAX_OPEN_ACTIONS {
        return Err(WalletError::Forbidden(format!(
            "at most {MAX_OPEN_ACTIONS} actions can be open at once"
        )));
    }
    if !approves
        && open.iter().filter(|(_, p)| p.proposed_by == caller).count() >= MAX_OPEN_BY_PROPOSER
    {
        return Err(WalletError::Forbidden(format!(
            "a proposer can have at most {MAX_OPEN_BY_PROPOSER} actions open at once"
        )));
    }
    let terms = match &spend {
        Some((chain, amount)) => {
            let spent = spent(chain, now, None);
            let spend = Spend {
                chain,
                amount,
                spent: &spent,
            };
            policy::terms(&governance.policy, ActionKind::Transfer, Some(&spend))
        }
        None => policy::terms(&governance.policy, action.kind(), None),
    };
    let approvals = if approves { vec![caller] } else { vec![] };
    let pending = StoredPending::new(action, caller, approvals, terms, APPROVAL_WINDOW_NS, now);
    let id = storage::add_action(pending.clone());
    audit(caller, AuditEvent::Proposed(id), now);
    Ok(pending.view(id, now))
}

/// Asks to join as a viewer, or as a proposer when `role` says so. It is an
/// `AddMember` action that the members approve like any other. Asking again
/// while a request is open returns it.
#[ic_cdk::update(guard = "access::claimed")]
fn request_join(label: String, role: Option<Role>) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let role = role.unwrap_or(Role::Viewer);
    if !matches!(role, Role::Viewer | Role::Proposer) {
        return Err(WalletError::Forbidden(
            "a join request asks to be a viewer or a proposer".to_string(),
        ));
    }
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
        role,
        label,
    };
    governance.check(&action, caller, now)?;
    let terms = policy::terms(&governance.policy, ActionKind::AddMember, None);
    let pending = StoredPending::new(action, caller, vec![], terms, JOIN_WINDOW_NS, now);
    let id = storage::add_action(pending.clone());
    audit(caller, AuditEvent::JoinRequested(caller), now);
    Ok(pending.view(id, now))
}

#[ic_cdk::update(guard = "access::approver")]
fn approve(id: ActionId) -> Result<Pending, WalletError> {
    change_action(id, |pending, caller, now| {
        let before = pending.approvals.len();
        pending.approve(id, caller, now)?;
        // Approving twice changes nothing, so it logs nothing.
        if pending.approvals.len() > before {
            audit(caller, AuditEvent::Approved(id), now);
        }
        Ok(())
    })
}

#[ic_cdk::update(guard = "access::owner")]
fn reject(id: ActionId) -> Result<Pending, WalletError> {
    // An install past its deadline ends before its action can be rejected.
    settle_upgrade(ic_cdk::api::time());
    change_action(id, |pending, caller, now| {
        pending.reject(id, caller, now)?;
        audit(caller, AuditEvent::Rejected(id), now);
        // The release it staged will not be installed.
        let mut upgrade = storage::upgrade();
        if upgrade.drop_staging(id) {
            storage::set_upgrade(upgrade);
            schedule_timer(now);
        }
        Ok(())
    })
}

/// Runs an action that has its quorum once its timelock has passed. An
/// action that no longer fits the members or the policy ends `Failed`.
#[ic_cdk::update(guard = "access::approver")]
async fn execute(id: ActionId) -> Result<Pending, WalletError> {
    let now = ic_cdk::api::time();
    // Nothing runs while an install is on its way, nor a second upgrade.
    check_not_installing(now)?;
    let mut pending = stored_action(id)?;
    if let Err(error) = pending.check_executable(id, now) {
        storage::set_action(id, pending);
        return Err(error);
    }
    if let Action::Upgrade {
        version,
        wasm_sha256,
    } = pending.action.clone()
    {
        return execute_upgrade(id, pending, version, wasm_sha256, now).await;
    }
    if let Action::HandOverControl {
        controller,
        controller_module_hash,
        controller_policy_sha256,
    } = pending.action.clone()
    {
        return execute_hand_over(
            id,
            pending,
            controller,
            controller_module_hash,
            controller_policy_sha256,
        )
        .await;
    }
    if let Action::Transfer(request) = pending.action.clone() {
        // Accounts are never deleted, so the account it was proposed from is
        // still there.
        let account = storage::account(request.from);
        return match account.as_ref().map(|account| &account.chain) {
            Some(Chain::Bitcoin(network)) => {
                send_bitcoin(id, pending, request, account.clone(), *network, now).await
            }
            _ => send(id, pending, request, now).await,
        };
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
    audit(ic_cdk::api::msg_caller(), AuditEvent::Executed(id), now);
    Ok(pending.view(id, now))
}

/// Executes a transfer with one ledger call, and one call at a time. A
/// transfer under the daily limit is checked against it again, unless a
/// call that may have run is being retried: that one only finds out.
async fn send(
    id: ActionId,
    mut pending: StoredPending,
    request: TransferRequest,
    now: u64,
) -> Result<Pending, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let retry = match pending.transfer_retry(id, now) {
        Ok(retry) => retry,
        Err(error) => {
            storage::set_action(id, pending);
            return Err(error);
        }
    };
    // Accounts are never deleted, so the account it was proposed from is
    // still there.
    let account = storage::account(request.from);
    let checked = transfers::check(&request, account.as_ref()).and_then(|to| {
        let account = account.clone().expect("a checked transfer has its account");
        if !retry && pending.daily_limit == Some(true) {
            fits_daily_limit(id, &account.chain, &request.amount, now)?;
        }
        Ok((account, to))
    });
    let (account, to) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            pending.finish(Err(reason(&error)));
            storage::set_action(id, pending.clone());
            audit(caller, AuditEvent::Executed(id), now);
            return Ok(pending.view(id, now));
        }
    };
    let ledger = accounts::ledger(&account.chain)?;
    let created_at_ns = pending.begin_transfer(now, || storage::transfer_time(now));
    storage::set_action(id, pending);
    let in_flight = InFlight {
        id: Some(id),
        execution: None,
    };
    let arg = transfers::ledger_arg(&request, &account, to, created_at_ns);
    let result = ledger::transfer(ledger, arg).await;

    // Other calls ran meanwhile; none of them changes an action in flight.
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    let ended = pending.end_transfer(
        transfers::read_reply(result),
        request.from,
        account.chain.clone(),
        now,
    );
    if let Ok(Some(block)) = &ended {
        storage::add_transfer(StoredTransfer {
            account: request.from,
            chain: account.chain,
            to: request.to,
            amount: request.amount,
            tx_id: block.0.to_str_radix(10),
            at_ns: now,
            action: id,
            fee: None,
        });
    }
    storage::set_action(id, pending.clone());
    if !matches!(pending.state, ActionState::Waiting | ActionState::Ready) {
        audit(caller, AuditEvent::Executed(id), now);
    }
    in_flight.returned();
    ended.map(|_| pending.view(id, now))
}

/// How a step of a Bitcoin transfer or an upgrade failed.
enum Step {
    /// The action cannot run: it ends `Failed` with this reason.
    Fails(String),
    /// This execution stops; the action stays ready.
    Stops(WalletError),
}

impl From<WalletError> for Step {
    fn from(error: WalletError) -> Self {
        Step::Stops(error)
    }
}

fn chain_error(reason: String) -> Step {
    Step::Stops(WalletError::ChainError(reason))
}

/// Executes a Bitcoin transfer: builds its transaction once, signs it, and
/// hands it to the Bitcoin canister, one execution at a time. Until the
/// transaction may have been sent, the transfer is checked again, against
/// the daily limit too, and the daily limit once more just before the send.
async fn send_bitcoin(
    id: ActionId,
    mut pending: StoredPending,
    request: TransferRequest,
    account: Option<StoredAccount>,
    network: BitcoinNetwork,
    now: u64,
) -> Result<Pending, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let account = account.expect("a Bitcoin transfer has its account");
    let execution = match pending.begin_bitcoin(id, now) {
        Ok(execution) => execution,
        Err(error) => {
            storage::set_action(id, pending);
            return Err(error);
        }
    };
    if !pending.may_have_sent() {
        let checked = check_transfer(&request, Some(&account)).and_then(|spend| {
            let spend = match pending.bitcoin_tx() {
                Some(tx) => request.amount.clone() + tx.fee,
                None => spend,
            };
            if pending.daily_limit == Some(true) {
                fits_daily_limit(id, &account.chain, &spend, now)?;
            }
            Ok(())
        });
        if let Err(error) = checked {
            pending.end_bitcoin_call();
            pending.finish(Err(reason(&error)));
            storage::set_action(id, pending.clone());
            audit(caller, AuditEvent::Executed(id), now);
            return Ok(pending.view(id, now));
        }
    }
    storage::set_action(id, pending);
    let in_flight = InFlight {
        id: Some(id),
        execution: Some(execution),
    };
    let result = bitcoin_steps(id, execution, &request, &account, network).await;

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    let ours = pending.owns_bitcoin(execution);
    let result = match result {
        // The steps recorded it `Executed`.
        Ok(()) => Ok(()),
        // Nothing awaited since the step that failed, so the action is
        // still open and this execution's.
        Err(Step::Fails(reason)) if ours => {
            pending.finish(Err(reason));
            Ok(())
        }
        Err(Step::Fails(reason)) => Err(WalletError::ChainError(reason)),
        Err(Step::Stops(error)) => Err(error),
    };
    if ours {
        pending.end_bitcoin_call();
        storage::set_action(id, pending.clone());
    }
    if result.is_ok() {
        audit(caller, AuditEvent::Executed(id), now);
    }
    in_flight.returned();
    result.map(|()| pending.view(id, now))
}

/// Re-reads action `id` after a call and goes on with `execution`, if it
/// still owns the transfer.
fn resume(id: ActionId, execution: u64) -> Result<StoredPending, Step> {
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    pending.resume_bitcoin(id, execution, now)?;
    storage::set_action(id, pending.clone());
    Ok(pending)
}

/// The steps of execution `execution` that call out: building the
/// transaction if it is not built yet, signing it if it is not signed, and
/// sending it. A transaction that was sent is recorded `Executed` here.
async fn bitcoin_steps(
    id: ActionId,
    execution: u64,
    request: &TransferRequest,
    account: &StoredAccount,
    network: BitcoinNetwork,
) -> Result<(), Step> {
    let address = account.bitcoin_address()?;
    let own = address.script_pubkey();
    let mut pending = resume(id, execution)?;
    if pending.bitcoin_tx().is_none() {
        let checked = btc::check(request, network)?;
        let to = checked.to.script_pubkey();
        let fee = match checked.fee {
            Some(total) => btc::Fee::Total(total),
            None => {
                let percentiles = bitcoin_canister::fee_percentiles(network)
                    .await
                    .map_err(chain_error)?;
                btc::Fee::Rate(btc::fee_rate(&percentiles))
            }
        };
        let coins = bitcoin_canister::coins(network, address.to_string())
            .await
            .map_err(chain_error)?;
        pending = resume(id, execution)?;
        let now = ic_cdk::api::time();
        // Expired actions release the outputs they reserved.
        storage::close_expired(now);
        let coins = coins
            .into_iter()
            .filter(|coin| !storage::is_reserved(&coin.key()))
            .collect();
        let plan = btc::plan(coins, checked.amount, &to, &own, fee).map_err(Step::Fails)?;
        if pending.daily_limit == Some(true) {
            let spend = Nat::from(checked.amount + plan.fee);
            fits_daily_limit(id, &account.chain, &spend, now)
                .map_err(|error| Step::Fails(reason(&error)))?;
        }
        let tx = btc::unsigned(&plan, checked.amount, &to, &own);
        storage::reserve(&btc::spent_keys(&tx), id);
        pending.bitcoin_built(BitcoinTx {
            unsigned: ByteBuf::from(btc::to_bytes(&tx)),
            values: plan.inputs.iter().map(|coin| coin.value).collect(),
            fee: plan.fee,
            signed: None,
            sent: false,
        });
        storage::set_action(id, pending.clone());
    }
    let tx = pending
        .bitcoin_tx()
        .cloned()
        .expect("the transaction is built");
    let unsigned = btc::from_bytes(&tx.unsigned).map_err(chain_error)?;
    let signed = match &tx.signed {
        Some(signed) => btc::from_bytes(signed).map_err(chain_error)?,
        None => {
            let hashes = btc::sighashes(&unsigned, &tx.values, &own).map_err(chain_error)?;
            let mut signatures = Vec::with_capacity(hashes.len());
            for hash in &hashes {
                let signature = signer::sign(network, account.index, *hash)
                    .await
                    .map_err(chain_error)?;
                signatures.push(signature);
                pending = resume(id, execution)?;
            }
            let public_key = account
                .public_key
                .as_ref()
                .map_or(&[][..], |key| key.as_slice());
            let signed = btc::signed(unsigned.clone(), &hashes, &signatures, public_key)
                .map_err(chain_error)?;
            pending.bitcoin_signed(btc::to_bytes(&signed));
            storage::set_action(id, pending.clone());
            signed
        }
    };
    // A policy that changed while the transaction was signed applies, unless
    // it may have been sent already: then only the same one can go.
    if !pending.may_have_sent() && pending.daily_limit == Some(true) {
        let spend = request.amount.clone() + tx.fee;
        fits_daily_limit(id, &account.chain, &spend, ic_cdk::api::time())
            .map_err(|error| Step::Fails(reason(&error)))?;
    }
    pending.bitcoin_sending();
    storage::set_action(id, pending);
    let result = bitcoin_canister::send(network, btc::to_bytes(&signed)).await;

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    if !pending.owns_bitcoin(execution) {
        // A later execution sends the same transaction.
        return Err(Step::Stops(WalletError::InvalidArgument(format!(
            "another execution of action {id} took over"
        ))));
    }
    let tx_id = btc::txid(&signed);
    let ended = pending.end_bitcoin_send(
        result,
        request.from,
        account.chain.clone(),
        tx_id.clone(),
        now,
    );
    if ended.is_ok() {
        storage::add_transfer(StoredTransfer {
            account: request.from,
            chain: account.chain.clone(),
            to: request.to.clone(),
            amount: request.amount.clone(),
            tx_id,
            at_ns: now,
            action: id,
            fee: Some(Nat::from(tx.fee)),
        });
    }
    storage::set_action(id, pending);
    ended.map_err(Step::Stops)
}

/// Marks a transfer's ledger call as one that may have run unseen, or a
/// Bitcoin execution as one that will not go on, unless the code after its
/// calls returns: when that code traps, ic-cdk drops the call's future, and
/// with it this guard, and keeps what the guard writes.
struct InFlight {
    id: Option<ActionId>,
    /// The execution of a Bitcoin transfer.
    execution: Option<u64>,
}

impl InFlight {
    fn returned(mut self) {
        self.id = None;
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(id) = self.id
            && let Some(mut pending) = storage::action(id)
        {
            match self.execution {
                Some(execution) => pending.lose_bitcoin_call(execution),
                None => pending.lose_transfer_call(),
            }
            storage::set_action(id, pending);
        }
    }
}

/// The release the channel published last, when the wallet would install
/// it: signed by enough of the release signer keys the wallet pins, and
/// tested to upgrade from the module the wallet runs. `None` when the
/// channel has no release, or the wallet runs its latest.
#[ic_cdk::update(guard = "access::member")]
async fn release_available() -> Result<Option<ReleaseManifest>, WalletError> {
    check_not_installing(ic_cdk::api::time())?;
    let config = installed_config();
    let latest = channel::latest(config.release_channel)
        .await
        .map_err(WalletError::ChainError)?;
    let Some(manifest) = latest else {
        return Ok(None);
    };
    let module_hash = management::itself()
        .await
        .map_err(WalletError::ChainError)?
        .module_hash;
    if module_hash.as_deref() == Some(manifest.wasm_sha256.as_slice()) {
        return Ok(None);
    }
    upgrade::check_release(&manifest, &release_signers(&config), module_hash.as_deref())
        .map_err(WalletError::ReleaseRejected)?;
    Ok(Some(manifest))
}

#[ic_cdk::query(guard = "access::member")]
fn upgrade_status() -> UpgradeState {
    storage::upgrade().view(ic_cdk::api::time())
}

fn installed_config() -> storage::Config {
    storage::config().unwrap_or_else(|| ic_cdk::trap("the wallet has no configuration"))
}

/// The release signer keys and threshold the wallet was installed with,
/// which `init` checked.
fn release_signers(config: &storage::Config) -> release::Signers {
    release::Signers::new(&config.release_signers, config.release_threshold)
        .unwrap_or_else(|reason| ic_cdk::trap(format!("the stored release signers: {reason}")))
}

/// Refuses while an install is on its way. An install whose deadline passed
/// failed: that is written down first, so a timer that never ran keeps
/// nothing waiting.
fn check_not_installing(now: u64) -> Result<(), WalletError> {
    settle_upgrade(now);
    if storage::upgrade().is_installing() {
        return Err(WalletError::UpgradeInProgress);
    }
    Ok(())
}

/// Ends an install the old version outlived past its deadline.
fn settle_upgrade(now: u64) {
    let mut upgrade = storage::upgrade();
    if let Some((id, version)) = upgrade.check_deadline(now) {
        storage::set_upgrade(upgrade);
        let reason = upgrade::failure(&version);
        end_upgrade(id, Err(reason), AuditEvent::UpgradeFailed(version), now);
        schedule_timer(now);
    }
}

/// Ends upgrade action `id` with `result`, and logs it and `event` as the
/// wallet's own doing.
fn end_upgrade(id: Option<ActionId>, result: Result<Outcome, String>, event: AuditEvent, now: u64) {
    if let Some(id) = id
        && let Some(mut pending) = storage::action(id)
    {
        pending.finish(result);
        storage::set_action(id, pending);
        audit_by_wallet(AuditEvent::Executed(id), now);
    }
    audit_by_wallet(event, now);
}

/// Executes an upgrade: checks the release the channel published under
/// `version` against the action and the wallet, stages its chunks in the
/// wallet's chunk store, and sends the install. One execution at a time; a
/// later one goes on where an earlier one stopped.
async fn execute_upgrade(
    id: ActionId,
    pending: StoredPending,
    version: String,
    wasm_sha256: ByteBuf,
    now: u64,
) -> Result<Pending, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let mut upgrade = storage::upgrade();
    let execution = upgrade.begin(now)?;
    storage::set_upgrade(upgrade);
    storage::set_action(id, pending);
    let in_flight = Staging(Some(execution));
    let result = upgrade_steps(id, execution, &version, &wasm_sha256, caller).await;

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    let mut upgrade = storage::upgrade();
    let ours = upgrade.owns(execution);
    let result = match result {
        // The install is on its way.
        Ok(()) => Ok(()),
        // Nothing awaited since the step that failed, so the action is still
        // open and this execution's.
        Err(Step::Fails(reason)) if ours => {
            pending.finish(Err(reason));
            audit(caller, AuditEvent::Executed(id), now);
            Ok(())
        }
        Err(Step::Fails(reason)) => Err(WalletError::ReleaseRejected(reason)),
        Err(Step::Stops(error)) => Err(error),
    };
    // A release whose action failed, expired or was rejected meanwhile will
    // not be installed.
    if !pending.is_open(now) {
        upgrade.drop_staging(id);
    }
    upgrade.end(execution);
    storage::set_upgrade(upgrade);
    storage::set_action(id, pending.clone());
    schedule_timer(now);
    in_flight.returned();
    result.map(|()| pending.view(id, now))
}

/// Re-reads upgrade action `id` after a call and goes on with
/// `execution`, if the action may still run and the execution still owns
/// the staging.
fn resume_upgrade(id: ActionId, execution: u64) -> Result<upgrade::StoredUpgrade, Step> {
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    if let Err(error) = pending.check_executable(id, now) {
        storage::set_action(id, pending);
        return Err(Step::Stops(error));
    }
    let mut upgrade = storage::upgrade();
    upgrade.resume(execution, now)?;
    storage::set_upgrade(upgrade.clone());
    Ok(upgrade)
}

/// The steps of execution `execution` that call out, then the install.
async fn upgrade_steps(
    id: ActionId,
    execution: u64,
    version: &str,
    wasm_sha256: &[u8],
    caller: Principal,
) -> Result<(), Step> {
    let config = installed_config();
    let manifest = channel::manifest(config.release_channel, version)
        .await
        .map_err(chain_error)?;
    resume_upgrade(id, execution)?;
    let manifest = manifest.ok_or_else(|| {
        Step::Stops(WalletError::ReleaseRejected(format!(
            "the release channel has no release {version}"
        )))
    })?;
    let itself = management::itself().await.map_err(chain_error)?;
    resume_upgrade(id, execution)?;
    let module_hash = itself.module_hash;
    if manifest.version != version || manifest.wasm_sha256.as_slice() != wasm_sha256 {
        return Err(Step::Fails(format!(
            "release {version} on the release channel is not the Wasm the action names"
        )));
    }
    if module_hash.as_deref() == Some(wasm_sha256) {
        return Err(Step::Fails(format!(
            "the wallet already runs release {version}"
        )));
    }
    upgrade::check_release(&manifest, &release_signers(&config), module_hash.as_deref())
        .map_err(Step::Fails)?;
    if !itself.controls_itself {
        return Err(Step::Stops(WalletError::Forbidden(
            "the wallet is not its own controller, so the IC would refuse to install a release"
                .to_string(),
        )));
    }

    let total = u32::try_from(manifest.chunk_hashes.len()).expect("at most 100 chunks");
    let mut upgrade = storage::upgrade();
    let first = upgrade.stage(id, manifest.clone());
    // The IC refuses an install the wallet cannot pay for, and only the
    // one-way install's lost reply would hear it: stop before staging.
    enough_cycles(upgrade::cycles_to_upgrade(total - first))?;
    let clear = upgrade.clear_store;
    storage::set_upgrade(upgrade);
    if clear {
        management::clear_chunk_store().await.map_err(chain_error)?;
        let mut upgrade = resume_upgrade(id, execution)?;
        upgrade.clear_store = false;
        storage::set_upgrade(upgrade);
    }
    for index in first..total {
        let chunk = channel::chunk(config.release_channel, version, index)
            .await
            .map_err(chain_error)?;
        resume_upgrade(id, execution)?;
        let chunk = chunk.ok_or_else(|| {
            chain_error(format!(
                "the release channel has no chunk {index} of release {version}"
            ))
        })?;
        manifest
            .check_chunk(index as usize, &chunk)
            .map_err(|error| {
                Step::Fails(format!(
                    "the release channel served chunk {index} of release {version} out of \
                     place: {error:?}"
                ))
            })?;
        let stored = management::upload_chunk(chunk).await.map_err(chain_error)?;
        let mut upgrade = resume_upgrade(id, execution)?;
        if stored.as_slice() != manifest.chunk_hashes[index as usize].as_slice() {
            return Err(chain_error(format!(
                "the chunk store keeps chunk {index} under another hash"
            )));
        }
        upgrade.uploaded(index);
        storage::set_upgrade(upgrade);
        if index + 1 == total {
            audit(
                caller,
                AuditEvent::UpgradeStaged(version.to_string()),
                ic_cdk::api::time(),
            );
        }
    }

    // No await from here on: the install goes out with no call waiting.
    let now = ic_cdk::api::time();
    let mut upgrade = resume_upgrade(id, execution)?;
    let waiting = calls::in_flight();
    if waiting > 0 {
        return Err(Step::Stops(WalletError::InvalidArgument(format!(
            "release {version} is staged, but the wallet still waits for the answers to \
             {waiting} of its calls; execute the upgrade again once they return"
        ))));
    }
    enough_cycles(upgrade::INSTALL_CYCLES)?;
    let (chunk_hashes, module_hash) = match upgrade.install_hashes() {
        Some(hashes) if upgrade.is_staged() => hashes,
        _ => return Err(chain_error(format!("release {version} is not staged"))),
    };
    management::install(chunk_hashes, module_hash).map_err(chain_error)?;
    upgrade.installing(now);
    storage::set_upgrade(upgrade);
    let mut pending = stored_action(id)?;
    pending.install_sent();
    storage::set_action(id, pending);
    schedule_timer(now);
    Ok(())
}

/// Checks the form of a `HandOverControl`: the controller is another
/// canister, and both hashes are SHA-256 hashes.
fn check_hand_over(
    controller: &Principal,
    module_hash: &[u8],
    policy_sha256: &[u8],
) -> Result<(), WalletError> {
    if !recovery::is_canister_id(controller) || *controller == ic_cdk::api::canister_self() {
        return Err(WalletError::InvalidArgument(
            "the controller must be another canister".to_string(),
        ));
    }
    if module_hash.len() != 32 || policy_sha256.len() != 32 {
        return Err(WalletError::InvalidArgument(
            "controller_module_hash and controller_policy_sha256 must be 32 bytes".to_string(),
        ));
    }
    Ok(())
}

thread_local! {
    /// Whether a `HandOverControl` execution waits for an answer. On the
    /// heap: no install goes out while it waits, since its calls count as
    /// in flight.
    static HANDING_OVER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks a `HandOverControl` execution in flight until it returns, or until
/// code after one of its calls traps: ic-cdk then drops the call's future,
/// and with it this guard, and keeps what the guard writes.
struct HandingOver;

impl HandingOver {
    fn start() -> Result<HandingOver, WalletError> {
        if HANDING_OVER.get() {
            return Err(WalletError::Forbidden(
                "a HandOverControl action is executing; try again once it returns".to_string(),
            ));
        }
        HANDING_OVER.set(true);
        Ok(HandingOver)
    }
}

impl Drop for HandingOver {
    fn drop(&mut self) {
        HANDING_OVER.set(false);
    }
}

/// Hands the wallet to a recovery controller (decision 0009). A check that
/// fails ends the action `Failed`; a call that fails leaves it ready, and
/// an execution after a hand-over whose answer was lost finds the wallet
/// already handed over and ends it `Executed`.
async fn execute_hand_over(
    id: ActionId,
    pending: StoredPending,
    controller: Principal,
    module_hash: ByteBuf,
    policy_sha256: ByteBuf,
) -> Result<Pending, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let in_flight = HandingOver::start()?;
    storage::set_action(id, pending);
    let result = hand_over_steps(id, controller, &module_hash, &policy_sha256).await;
    drop(in_flight);

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    match result {
        // The wallet belongs to the controller now, whatever happened to the
        // action meanwhile.
        Ok(()) => pending.finish(Ok(Outcome::Done)),
        Err(Step::Fails(reason)) if pending.is_open(now) => pending.finish(Err(reason)),
        Err(Step::Fails(reason)) => return Err(WalletError::Forbidden(reason)),
        Err(Step::Stops(error)) => return Err(error),
    }
    storage::set_action(id, pending.clone());
    audit(caller, AuditEvent::Executed(id), now);
    Ok(pending.view(id, now))
}

/// Re-reads `HandOverControl` action `id` after a call, and goes on only if
/// it may still run.
fn resume_hand_over(id: ActionId) -> Result<(), Step> {
    let mut pending = stored_action(id)?;
    if let Err(error) = pending.check_executable(id, ic_cdk::api::time()) {
        storage::set_action(id, pending);
        return Err(Step::Stops(error));
    }
    Ok(())
}

/// The checks of a `HandOverControl`, then its one change: the controller
/// must run the approved module, have been installed once and fresh with no
/// controller left, serve this wallet with the approved policy and have run
/// nothing yet, and run on this wallet's subnet with settings that cannot
/// strand the wallet.
async fn hand_over_steps(
    id: ActionId,
    controller: Principal,
    module_hash: &[u8],
    policy_sha256: &[u8],
) -> Result<(), Step> {
    let itself = ic_cdk::api::canister_self();
    let wallet = management::info(itself, 0).await.map_err(chain_error)?;
    resume_hand_over(id)?;
    if wallet.controllers == [controller] {
        return Ok(());
    }
    if !wallet.controllers.contains(&itself) {
        return Err(Step::Stops(WalletError::Forbidden(
            "the wallet is not its own controller, so it cannot hand itself over".to_string(),
        )));
    }
    let info = management::info(controller, recovery::history::MAX_CHANGES)
        .await
        .map_err(chain_error)?;
    resume_hand_over(id)?;
    recovery::history::check_fresh_controller(&info, module_hash)
        .map_err(|reason| Step::Fails(format!("controller {controller}: {reason}")))?;
    let state: recovery::controller::ControllerState =
        calls::call_without_args(controller, format!("controller {controller}"), "state")
            .await
            .map_err(|failure| chain_error(calls::reason(failure)))?;
    resume_hand_over(id)?;
    if state.wallet != itself {
        return Err(Step::Fails(format!(
            "controller {controller} serves wallet {}, not this one",
            state.wallet
        )));
    }
    if state.policy_sha256.as_slice() != policy_sha256 {
        return Err(Step::Fails(format!(
            "controller {controller} holds another policy than the approved one"
        )));
    }
    if let Some(next) = state.replaced_by {
        return Err(Step::Fails(format!(
            "controller {controller} handed this wallet to {next} already"
        )));
    }
    if state.policy_version != 1 || state.next_nonce != 1 {
        return Err(Step::Fails(format!(
            "controller {controller} has changed its policy or taken operations already; hand \
             the wallet only to a controller as its install left it"
        )));
    }
    // Nobody can change a controller's settings once it has no controller,
    // and a controller on another subnet adds one the wallet depends on.
    let health: recovery::controller::Health =
        calls::call_without_args(controller, format!("controller {controller}"), "health")
            .await
            .map_err(|failure| chain_error(calls::reason(failure)))?;
    resume_hand_over(id)?;
    health
        .check(ic_cdk::api::subnet_self())
        .map_err(|reason| Step::Fails(format!("controller {controller}: {reason}")))?;
    // One call sets exactly one controller, which drops the wallet itself.
    management::set_controllers(vec![controller])
        .await
        .map_err(|failure| match failure {
            CallFailure::NotRun(reason) => chain_error(reason),
            CallFailure::Unknown(reason) => chain_error(format!(
                "{reason}; the hand-over may have run: execute the action again to find out"
            )),
        })
}

/// Stops an upgrade unless the wallet can spend `needed` cycles above what it
/// keeps to stay running.
fn enough_cycles(needed: u128) -> Result<(), Step> {
    let available = ic_cdk::api::canister_liquid_cycle_balance();
    if available < needed {
        return Err(Step::Stops(WalletError::InsufficientCycles {
            available: Nat::from(available),
            needed: Nat::from(needed),
        }));
    }
    Ok(())
}

/// Marks an upgrade execution as one that will not go on, unless the code
/// after its calls returns: when that code traps, ic-cdk drops the call's
/// future, and with it this guard, and keeps what the guard writes.
struct Staging(Option<u64>);

impl Staging {
    fn returned(mut self) {
        self.0 = None;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(execution) = self.0 {
            let mut upgrade = storage::upgrade();
            upgrade.end(execution);
            storage::set_upgrade(upgrade);
        }
    }
}

/// Sets the IC's one timer for the next thing the upgrade waits for, or
/// turns it off.
fn schedule_timer(now: u64) {
    let at = storage::upgrade().wake_at(now).unwrap_or(0);
    ic_cdk::api::global_timer_set(at);
}

/// How often the timer tries again to clear the chunk store.
const CLEAR_RETRY_NS: u64 = 60 * 1_000_000_000;

/// Ends an install past its deadline, sends one again when its turn comes,
/// and clears the chunk store once no release needs it.
fn on_timer(now: u64) {
    settle_upgrade(now);
    resend_install(now);
    let mut upgrade = storage::upgrade();
    if upgrade.may_clear_store() {
        if management::clear_chunk_store_oneway().is_err() {
            ic_cdk::api::global_timer_set(now.saturating_add(CLEAR_RETRY_NS));
            return;
        }
        upgrade.clear_store = false;
        storage::set_upgrade(upgrade);
    }
    schedule_timer(now);
}

/// Sends the install on its way again when its turn comes. This version
/// still runs, so the IC refused the last send, as it does for a while after
/// another install, or has not run it yet; then the new version installs
/// again and its `post_upgrade` finds the upgrade done. A turn passes unused
/// while a call waits for its answer or the wallet cannot pay for the
/// install.
fn resend_install(now: u64) {
    let mut upgrade = storage::upgrade();
    if !upgrade.resend(now) {
        return;
    }
    if calls::in_flight() == 0
        && ic_cdk::api::canister_liquid_cycle_balance() >= upgrade::INSTALL_CYCLES
        && let Some((chunk_hashes, module_hash)) = upgrade.install_hashes()
    {
        // A send that cannot go out passes like an unused turn.
        let _ = management::install(chunk_hashes, module_hash);
    }
    storage::set_upgrade(upgrade);
}

#[cfg_attr(target_family = "wasm", unsafe(export_name = "canister_global_timer"))]
#[cfg_attr(
    not(target_family = "wasm"),
    unsafe(export_name = "canister_global_timer_host")
)]
fn global_timer() {
    ic_cdk::futures::internals::in_executor_context(|| on_timer(ic_cdk::api::time()));
}

/// Newest first: up to `limit` entries, at most `MAX_AUDIT_PAGE`, before
/// `before_seq`, or from the newest.
#[ic_cdk::query(guard = "access::member")]
fn audit_log(request: AuditRequest) -> Result<Vec<AuditEntry>, WalletError> {
    let limit = usize::from(request.limit).min(MAX_AUDIT_PAGE);
    Ok(storage::audit_page(request.before_seq, limit))
}

/// The most entries one `audit_log` call returns.
const MAX_AUDIT_PAGE: usize = 100;

/// Records a change made by `by`. Every call that changes state writes one
/// entry for it; a refused call writes none. An expiry is no one's act and is
/// not logged: the action itself shows it.
fn audit(by: Principal, event: AuditEvent, now: u64) {
    storage::append_audit(StoredAuditEntry {
        at_ns: now,
        by: Some(by),
        event,
    });
}

/// Records a change the wallet made itself: how an upgrade ended.
fn audit_by_wallet(event: AuditEvent, now: u64) {
    storage::append_audit(StoredAuditEntry {
        at_ns: now,
        by: None,
        event,
    });
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
        upgrade: storage::upgrade().view(ic_cdk::api::time()),
        release_channel: config.release_channel,
    }
}

/// The wallet's pages, certified. Anyone may call it, also before the claim.
#[ic_cdk::query]
fn http_request(request: ic_http_certification::HttpRequest<'static>) -> HttpResponse {
    let Some(certificate) = ic_cdk::api::data_certificate() else {
        ic_cdk::trap("http_request answers only as a query");
    };
    http::serve(&certificate, &request)
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
