//! Transfers: what one spends against its asset's daily limit, how
//! `execute` sends one, and how the wallet settles what it sent.
//!
//! A transfer from an Internet Computer account is one ledger call; one from
//! a Bitcoin account is one transaction, built once, signed, and handed to
//! the Bitcoin canister; one from an EVM account is one transaction, built
//! with its nonce, signed, and handed to the chain's providers through the
//! EVM RPC canister, and built again by each execution until a provider may
//! have seen it. An approver's `execute` starts any of them. The
//! wallet's timer then goes on with every transfer whose outcome it does
//! not know yet, so none waits for a member to come back:
//!
//! - a ledger call that may have run unseen is made again with the same
//!   `created_at_time`, while the ledger can still find the first;
//! - a Bitcoin transaction that may have been sent is watched until its
//!   inputs stay spent for `btc::SETTLE_CONFIRMATIONS` blocks, and sent
//!   again, less often each time, while they are not. Its inputs stay
//!   reserved until then, and an account's balance leaves them out and
//!   counts the transaction's change;
//! - an EVM transaction that may have been sent is watched, less often each
//!   time, until the chain finalizes a block that used its nonce and the
//!   providers have its receipt, which says whether it ran; it is sent
//!   again until the chain finalizes that block. Until the chain uses its
//!   nonce, it holds every later transaction from its account on its chain
//!   (see `evm_nonce`).

use crate::accounts::StoredAccount;
use crate::actions::{BitcoinTx, EvmTx, StoredPending, Tx};
use crate::btc::{Next, Utxos};
use crate::evm_rpc::BlockTag;
use crate::policy::{self, DAY_NS, Spend};
use crate::storage::{self, StoredTransfer};
use crate::{
    AccountId, AccountKind, Action, ActionId, ActionKind, ActionState, AuditEvent, BitcoinNetwork,
    Chain, Pending, Step, TransferRequest, WalletError, assets, bitcoin_canister, btc, chain_error,
    evm, evm_rpc, icrc, keys, log, reason, schedule_timer, stored_action,
};
use candid::{Nat, Principal};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// How often the timer settles while a transfer needs it.
pub const SETTLE_EVERY_NS: u64 = 10 * 60 * 1_000_000_000;

/// Checks a transfer from `account` and returns what it spends against its
/// asset's daily limit: the amount, and for Bitcoin and EVM the fee the
/// proposer fixed.
pub fn check_transfer(
    request: &TransferRequest,
    account: &StoredAccount,
) -> Result<Nat, WalletError> {
    match account.kind() {
        AccountKind::InternetComputer => icrc::check(request).map(|_| request.amount.clone()),
        AccountKind::Bitcoin(network) => {
            let checked = btc::check(request, network)?;
            Ok(Nat::from(checked.amount + checked.fee.unwrap_or(0)))
        }
        AccountKind::Evm => {
            let checked = evm::check(request)?;
            Ok(checked.amount + checked.fee.unwrap_or_default())
        }
    }
}

/// What an open transfer of `asset` may still spend: the amount, and for
/// Bitcoin and EVM the most its transaction pays in fees once built, or the
/// fee its proposer fixed.
fn committed(pending: &StoredPending, request: &TransferRequest, asset: &Chain) -> Nat {
    let fee = match asset {
        Chain::Bitcoin(_) => pending.bitcoin_tx().map(Tx::fee),
        Chain::Evm(_) => pending.evm_tx().map(Tx::fee),
        _ => return request.amount.clone(),
    };
    request.amount.clone() + fee.or_else(|| request.fee.clone()).unwrap_or_default()
}

/// What the wallet sent of `asset` in the last 24 hours, and what its open
/// transfers under the daily limit may still send, apart from the action
/// `except`.
pub fn spent(asset: &Chain, now: u64, except: Option<ActionId>) -> Nat {
    let mut spent = storage::sent_since(asset, now.saturating_sub(DAY_NS));
    for (id, pending) in storage::open_actions(now) {
        if Some(id) == except || pending.daily_limit != Some(true) {
            continue;
        }
        if let Action::Transfer(request) = &pending.action
            && request.asset.as_ref() == Some(asset)
        {
            spent += committed(&pending, request, asset);
        }
    }
    spent
}

/// Checks that action `id`, a transfer the daily limit released, still fits
/// its asset's limit when it spends `amount`.
fn fits_daily_limit(
    id: ActionId,
    asset: &Chain,
    amount: &Nat,
    now: u64,
) -> Result<(), WalletError> {
    let spent = spent(asset, now, Some(id));
    let spend = Spend {
        chain: asset,
        amount,
        spent: &spent,
    };
    let terms = policy::terms(&storage::policy(), ActionKind::Transfer, Some(&spend));
    if terms.quorum > 1 || terms.timelock_ns > 0 {
        return Err(WalletError::PolicyViolation(
            "the transfer no longer fits its asset's daily limit; propose it again".to_string(),
        ));
    }
    Ok(())
}

/// Ends transfer `id` `Failed` with `error`, as `by` executed it.
fn fail(
    id: ActionId,
    mut pending: StoredPending,
    error: &WalletError,
    by: Option<Principal>,
    now: u64,
) -> Result<Pending, WalletError> {
    pending.finish(Err(reason(error)));
    storage::set_action(id, pending.clone());
    log(by, AuditEvent::Executed(id), now);
    Ok(pending.view(id, now))
}

/// Executes transfer `id`, which `check_executable` allowed, for `by`: an
/// approver, or the wallet itself (`None`) when it goes on with one.
pub async fn execute(
    id: ActionId,
    pending: StoredPending,
    request: TransferRequest,
    by: Option<Principal>,
    now: u64,
) -> Result<Pending, WalletError> {
    // Accounts are never deleted, and `propose` named the asset.
    let Some((from, account)) = storage::listed_account(request.from) else {
        let error = WalletError::NotFound(format!("account {}", request.from));
        return fail(id, pending, &error, by, now);
    };
    let asset = request.asset.clone();
    let result = match (account.kind(), asset) {
        (AccountKind::InternetComputer, Some(asset)) if assets::holds(account.kind(), &asset) => {
            send_ledger(id, pending, request, from, account, asset, by, now).await
        }
        (AccountKind::Bitcoin(_), Some(asset)) if assets::holds(account.kind(), &asset) => {
            send_tx::<BitcoinTx>(id, pending, request, from, account, asset, by, now).await
        }
        (AccountKind::Evm, Some(Chain::Evm(chain)))
            if evm::holds(storage::bitcoin_network(), chain) =>
        {
            let asset = Chain::Evm(chain);
            send_tx::<EvmTx>(id, pending, request, from, account, asset, by, now).await
        }
        (kind, asset) => {
            let error =
                WalletError::InvalidArgument(format!("a {kind:?} account does not send {asset:?}"));
            fail(id, pending, &error, by, now)
        }
    };
    let ended = ic_cdk::api::time();
    if storage::action(id).is_some_and(|pending| pending.needs_settling()) {
        settle_later(ended);
    }
    schedule_timer(ended);
    result
}

/// Executes a transfer with one ledger call, and one call at a time. A
/// transfer under the daily limit is checked against it again, unless a
/// call that may have run is being retried: that one only finds out.
#[allow(clippy::too_many_arguments)]
async fn send_ledger(
    id: ActionId,
    mut pending: StoredPending,
    request: TransferRequest,
    from: AccountId,
    account: StoredAccount,
    asset: Chain,
    by: Option<Principal>,
    now: u64,
) -> Result<Pending, WalletError> {
    let retry = match pending.transfer_retry(id, now) {
        Ok(retry) => retry,
        Err(error) => {
            storage::set_action(id, pending);
            return Err(error);
        }
    };
    let ledger = assets::ledger(&asset).expect("an Internet Computer account holds ledger assets");
    let checked = icrc::check(&request).and_then(|to| {
        if !retry && pending.daily_limit == Some(true) {
            fits_daily_limit(id, &asset, &request.amount, now)?;
        }
        Ok(to)
    });
    let to = match checked {
        Ok(to) => to,
        Err(error) => return fail(id, pending, &error, by, now),
    };
    let created_at_ns = pending.begin_transfer(now, || storage::transfer_time(now));
    storage::set_action(id, pending);
    let in_flight = InFlight {
        id: Some(id),
        lose: |pending, _| pending.lose_transfer_call(),
        execution: 0,
    };
    let arg = icrc::ledger_arg(&request, &account, to, created_at_ns);
    let result = icrc::transfer(ledger, arg).await;

    // Other calls ran meanwhile; none of them changes an action in flight.
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    let ended = pending.end_transfer(icrc::read_reply(result), from, asset.clone(), now);
    if let Ok(Some(block)) = &ended {
        storage::add_transfer(StoredTransfer {
            account: from,
            chain: asset,
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
        log(by, AuditEvent::Executed(id), now);
    }
    in_flight.returned();
    ended.map(|_| pending.view(id, now))
}

/// Executes a Bitcoin or EVM transfer: builds its transaction, signs it, and
/// sends it, one execution at a time. Until the transaction may have been
/// sent, the transfer is checked again, against the daily limit too, and
/// the daily limit once more just before the send.
#[allow(clippy::too_many_arguments)]
async fn send_tx<T: Tx>(
    id: ActionId,
    mut pending: StoredPending,
    request: TransferRequest,
    from: AccountId,
    account: StoredAccount,
    asset: Chain,
    by: Option<Principal>,
    now: u64,
) -> Result<Pending, WalletError> {
    let execution = match pending.begin_send::<T>(id, now) {
        Ok(execution) => execution,
        Err(error) => {
            storage::set_action(id, pending);
            return Err(error);
        }
    };
    if T::REBUILT_UNSENT {
        pending.unbuild_unsent::<T>();
    }
    if !pending.may_have_sent() {
        let checked = check_transfer(&request, &account).and_then(|spend| {
            let spend = match pending.tx::<T>() {
                Some(tx) => request.amount.clone() + tx.fee(),
                None => spend,
            };
            if pending.daily_limit == Some(true) {
                fits_daily_limit(id, &asset, &spend, now)?;
            }
            Ok(())
        });
        if let Err(error) = checked {
            pending.end_send_call::<T>();
            return fail(id, pending, &error, by, now);
        }
    }
    storage::set_action(id, pending);
    let in_flight = InFlight {
        id: Some(id),
        lose: StoredPending::lose_send_call::<T>,
        execution,
    };
    let result = match asset {
        Chain::Bitcoin(network) => {
            bitcoin_steps(id, execution, &request, from, &account, network, by).await
        }
        Chain::Evm(chain) => evm_steps(id, execution, &request, from, &account, chain, by).await,
        _ => unreachable!("only Bitcoin and EVM transfers send transactions"),
    };

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    let ours = pending.owns_send::<T>(execution);
    let result = match result {
        // The steps recorded it `Executed`.
        Ok(()) => Ok(()),
        // Nothing awaited since the step that failed, so the action is
        // still open and this execution's.
        Err(Step::Fails(reason)) if ours => {
            pending.finish(Err(reason));
            log(by, AuditEvent::Executed(id), now);
            Ok(())
        }
        Err(Step::Fails(reason)) => Err(WalletError::ChainError(reason)),
        Err(Step::Stops(error)) => Err(error),
    };
    if ours {
        pending.end_send_call::<T>();
        storage::set_action(id, pending.clone());
    }
    in_flight.returned();
    result.map(|()| pending.view(id, now))
}

/// Re-reads action `id` after a call and goes on with `execution`, if it
/// still owns the transfer.
fn resume<T: Tx>(id: ActionId, execution: u64) -> Result<StoredPending, Step> {
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    pending.resume_send::<T>(id, execution, now)?;
    storage::set_action(id, pending.clone());
    Ok(pending)
}

/// The steps of Bitcoin execution `execution` that call out: building the
/// transaction if it is not built yet, signing it if it is not signed, and
/// sending it. A transaction the Bitcoin canister took is recorded
/// `Executed` here.
async fn bitcoin_steps(
    id: ActionId,
    execution: u64,
    request: &TransferRequest,
    from: AccountId,
    account: &StoredAccount,
    network: BitcoinNetwork,
    by: Option<Principal>,
) -> Result<(), Step> {
    let asset = Chain::Bitcoin(network);
    let address = account.bitcoin_address()?;
    let own = address.script_pubkey();
    let mut pending = resume::<BitcoinTx>(id, execution)?;
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
        let utxos = bitcoin_canister::coins(network, address.to_string())
            .await
            .map_err(chain_error)?;
        pending = resume::<BitcoinTx>(id, execution)?;
        let now = ic_cdk::api::time();
        // Expired actions release the outputs they reserved.
        storage::close_expired(now);
        let coins = utxos
            .coins
            .into_iter()
            .filter(|coin| !storage::is_reserved(&coin.key()))
            .collect();
        let plan = btc::plan(coins, checked.amount, &to, &own, fee).map_err(Step::Fails)?;
        if pending.daily_limit == Some(true) {
            let spend = Nat::from(checked.amount + plan.fee);
            fits_daily_limit(id, &asset, &spend, now)
                .map_err(|error| Step::Fails(reason(&error)))?;
        }
        let tx = btc::unsigned(&plan, checked.amount, &to, &own);
        storage::reserve(&btc::spent_keys(&tx), id);
        pending.built(BitcoinTx::new(
            btc::to_bytes(&tx),
            plan.inputs.iter().map(|coin| coin.value).collect(),
            plan.fee,
        ));
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
            let key = keys::bitcoin(network, account.index);
            let hashes = btc::sighashes(&unsigned, &tx.values, &own).map_err(chain_error)?;
            let mut signatures = Vec::with_capacity(hashes.len());
            for hash in &hashes {
                let signature = keys::sign(&key, *hash).await.map_err(chain_error)?;
                signatures.push(signature);
                pending = resume::<BitcoinTx>(id, execution)?;
            }
            let public_key = account
                .public_key
                .as_ref()
                .map_or(&[][..], |key| key.as_slice());
            let signed = btc::signed(unsigned.clone(), &hashes, &signatures, public_key)
                .map_err(chain_error)?;
            pending.signed::<BitcoinTx>(btc::to_bytes(&signed));
            storage::set_action(id, pending.clone());
            signed
        }
    };
    // A policy that changed while the transaction was signed applies, unless
    // it may have been sent already: then only the same one can go.
    if !pending.may_have_sent() && pending.daily_limit == Some(true) {
        let spend = request.amount.clone() + tx.fee;
        fits_daily_limit(id, &asset, &spend, ic_cdk::api::time())
            .map_err(|error| Step::Fails(reason(&error)))?;
    }
    pending.sending::<BitcoinTx>(ic_cdk::api::time());
    storage::set_action(id, pending);
    let result = bitcoin_canister::send(network, btc::to_bytes(&signed)).await;

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    if !pending.owns_send::<BitcoinTx>(execution) {
        // A later execution sends the same transaction, or the wallet saw
        // the network confirm it.
        return Err(Step::Stops(WalletError::InvalidArgument(format!(
            "another execution of action {id} took over"
        ))));
    }
    let tx_id = btc::txid(&signed);
    let ended =
        pending.end_send::<BitcoinTx>(result.into(), from, asset.clone(), tx_id.clone(), now);
    if ended.is_ok() {
        storage::add_transfer(StoredTransfer {
            account: from,
            chain: asset,
            to: request.to.clone(),
            amount: request.amount.clone(),
            tx_id,
            at_ns: now,
            action: id,
            fee: Some(Nat::from(tx.fee)),
        });
        log(by, AuditEvent::Executed(id), now);
    }
    storage::set_action(id, pending);
    ended.map_err(Step::Stops)
}

/// The steps of EVM execution `execution` that call out: building the
/// transaction if it is not built yet, signing it if it is not signed, and
/// sending it. A transaction a provider took is recorded `Executed` here.
async fn evm_steps(
    id: ActionId,
    execution: u64,
    request: &TransferRequest,
    from: AccountId,
    account: &StoredAccount,
    chain: u64,
    by: Option<Principal>,
) -> Result<(), Step> {
    let asset = Chain::Evm(chain);
    let address = account.evm_address()?;
    let mut pending = resume::<EvmTx>(id, execution)?;
    if pending.evm_tx().is_none() {
        let checked = evm::check(request)?;
        // Fails early while another execution sends from the account.
        evm_nonce(id, from, chain, None, ic_cdk::api::time())?;
        let estimates = evm_rpc::fee_estimates(chain).await.map_err(chain_error)?;
        resume::<EvmTx>(id, execution)?;
        let gas = evm_rpc::estimate_gas(chain, &address, &checked.to, &checked.amount)
            .await
            .map_err(chain_error)?;
        resume::<EvmTx>(id, execution)?;
        let gas_limit = evm::gas_limit(&gas).map_err(Step::Fails)?;
        let fees = evm::fees(chain, &estimates).map_err(chain_error)?;
        let mut tx = evm::Transaction {
            chain_id: chain,
            // The longest a nonce encodes, until the chain says which.
            nonce: u64::MAX,
            max_priority_fee_per_gas: fees.max_priority_fee_per_gas.clone(),
            max_fee_per_gas: fees.max_fee_per_gas.clone(),
            gas_limit,
            to: checked.to,
            value: checked.amount.clone(),
        };
        let beyond_gas = if evm::charges_beyond_gas(chain) {
            let named = evm_rpc::beyond_gas(chain, tx.unsigned_len(), gas_limit)
                .await
                .map_err(chain_error)?;
            resume::<EvmTx>(id, execution)?;
            named * 2u8
        } else {
            Nat::from(0u8)
        };
        let fees = match &checked.fee {
            None => fees,
            Some(fee) if *fee <= beyond_gas => {
                return Err(Step::Fails(format!(
                    "the fee cap of {} wei does not cover the {} chain {chain} may charge \
                     beyond gas",
                    fee.0, beyond_gas.0
                )));
            }
            Some(fee) => {
                let capped = evm::capped(fees, gas_limit, &(fee.clone() - beyond_gas.clone()));
                // Signed, it would hold its nonce, and every later transfer
                // from the account on the chain, until the fees fall.
                if !evm::runs_now(chain, &capped) {
                    return Err(chain_error(format!(
                        "the fee cap of {} wei pays at most {} wei per gas, and chain {chain} \
                         charges {} wei per gas now; execute it again once its fees are lower, \
                         or propose the transfer again with a higher cap",
                        fee.0, capped.max_fee_per_gas.0, capped.base_fee_per_gas.0
                    )));
                }
                capped
            }
        };
        tx.max_fee_per_gas = fees.max_fee_per_gas;
        tx.max_priority_fee_per_gas = fees.max_priority_fee_per_gas;
        // The account's count and balance are read last, so they are as
        // fresh as they can be. A transaction of the account settling while
        // they are read could leave its nonce out of the count: then this
        // one is built again.
        let settled = settled_evm(from, chain);
        let count = evm_rpc::transaction_count(chain, &address, BlockTag::Latest)
            .await
            .map_err(chain_error)?;
        resume::<EvmTx>(id, execution)?;
        let balance = evm_rpc::balance(chain, &address)
            .await
            .map_err(chain_error)?;
        pending = resume::<EvmTx>(id, execution)?;
        if settled_evm(from, chain) != settled {
            return Err(chain_error(format!(
                "a transaction from account {from} on chain {chain} settled while this one was \
                 built; execute it again"
            )));
        }
        let now = ic_cdk::api::time();
        let spend = checked.amount + tx.max_fee() + beyond_gas.clone();
        if balance < spend {
            return Err(Step::Fails(format!(
                "account {from} holds {} wei on chain {chain}, and the transfer needs {} with \
                 its fees",
                balance.0, spend.0
            )));
        }
        if pending.daily_limit == Some(true) {
            fits_daily_limit(id, &asset, &spend, now)
                .map_err(|error| Step::Fails(reason(&error)))?;
        }
        tx.nonce = evm_nonce(id, from, chain, Some(count), now)?;
        pending.built(EvmTx::new(&tx, beyond_gas));
        storage::set_action(id, pending.clone());
    }
    let tx = pending.evm_tx().cloned().expect("the transaction is built");
    let transaction = tx.transaction();
    let signed = match &tx.signed {
        Some(signed) => signed.to_vec(),
        None => {
            let key = keys::evm(storage::bitcoin_network(), account.index);
            let sighash = transaction.sighash();
            let signature = keys::sign(&key, sighash).await.map_err(chain_error)?;
            pending = resume::<EvmTx>(id, execution)?;
            let public_key = account
                .public_key
                .as_ref()
                .map_or(&[][..], |key| key.as_slice());
            let signature =
                evm::signature(&signature, &sighash, public_key).map_err(chain_error)?;
            let signed = transaction.signed(&signature);
            pending.signed::<EvmTx>(signed.clone());
            storage::set_action(id, pending.clone());
            signed
        }
    };
    // A policy that changed while the transaction was signed applies, unless
    // it may have been sent already: then only the same one can go.
    if !pending.may_have_sent() && pending.daily_limit == Some(true) {
        let spend = request.amount.clone() + tx.max_fee();
        fits_daily_limit(id, &asset, &spend, ic_cdk::api::time())
            .map_err(|error| Step::Fails(reason(&error)))?;
    }
    let sent_before = tx.sent;
    pending.sending::<EvmTx>(ic_cdk::api::time());
    storage::set_action(id, pending);
    let result = evm_rpc::send(chain, &signed).await;

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    if !pending.owns_send::<EvmTx>(execution) {
        // A later execution sends the same transaction, or the wallet saw
        // the chain settle it.
        return Err(Step::Stops(WalletError::InvalidArgument(format!(
            "another execution of action {id} took over"
        ))));
    }
    let tx_id = evm::hash_text(&evm::tx_hash(&signed));
    let reply = evm_rpc::read_send(result, sent_before);
    let ended = pending.end_send::<EvmTx>(reply, from, asset.clone(), tx_id.clone(), now);
    if ended.is_ok() {
        storage::add_transfer(StoredTransfer {
            account: from,
            chain: asset,
            to: request.to.clone(),
            amount: request.amount.clone(),
            tx_id,
            at_ns: now,
            action: id,
            fee: Some(tx.max_fee()),
        });
        log(by, AuditEvent::Executed(id), now);
    }
    storage::set_action(id, pending);
    ended.map_err(Step::Stops)
}

/// The nonce of the transaction of action `id` from EVM account `from` on
/// `chain`, once the chain has used `count` nonces of the account: `count`,
/// once it used the nonce of every other transaction from the account there
/// that may have been sent and has not settled. Without a count, only
/// checks that no other execution is building and sending one, which holds
/// its nonce until it is sent.
fn evm_nonce(
    id: ActionId,
    from: AccountId,
    chain: u64,
    count: Option<u64>,
    now: u64,
) -> Result<u64, Step> {
    let mut ids: Vec<ActionId> = storage::open_actions(now)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    ids.extend(storage::unsettled());
    let mut built = Vec::new();
    for other in ids {
        if other == id {
            continue;
        }
        let Some(pending) = storage::action(other) else {
            continue;
        };
        let (Action::Transfer(request), Some(tx)) = (&pending.action, pending.evm_tx()) else {
            continue;
        };
        if request.from != from || tx.chain_id != chain {
            continue;
        }
        if !pending.may_have_sent() {
            // One no execution is sending is built again when it runs.
            if pending.executing::<EvmTx>(now) {
                return Err(chain_error(format!(
                    "action {other} is sending a transaction from account {from} on chain \
                     {chain}; execute this one again once it has"
                )));
            }
            continue;
        }
        if !tx.is_settled() {
            built.push((other, tx.nonce));
        }
    }
    let Some(count) = count else {
        return Ok(0);
    };
    evm::next_nonce(count, &built).map_err(|other| {
        chain_error(format!(
            "the transaction of action {other} from account {from} on chain {chain} has not \
             run yet; execute this one again once it has"
        ))
    })
}

/// Marks a transfer's ledger call as one that may have run unseen, or an
/// execution that sends a transaction as one that will not go on, unless
/// the code after its calls returns: when that code traps, ic-cdk drops the
/// call's future, and with it this guard, and keeps what the guard writes.
struct InFlight {
    id: Option<ActionId>,
    /// Records the loss in the action, given `execution`.
    lose: fn(&mut StoredPending, u64),
    /// The execution of a transfer that sends a transaction.
    execution: u64,
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
            (self.lose)(&mut pending, self.execution);
            storage::set_action(id, pending);
            // The timer settles it.
            schedule_timer(ic_cdk::api::time());
        }
    }
}

thread_local! {
    /// Whether the timer is settling now. On the heap: a new version starts
    /// with no task running.
    static SETTLING: Cell<bool> = const { Cell::new(false) };
    /// When the timer last started settling.
    static SETTLED_AT: Cell<u64> = const { Cell::new(0) };
    /// How many EVM transactions settled, by account and chain, since this
    /// version started. On the heap: a new version starts with no execution
    /// in flight.
    static EVM_SETTLED: RefCell<BTreeMap<(AccountId, u64), u64>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// How many transactions from EVM account `from` on `chain` settled since
/// this version started.
fn settled_evm(from: AccountId, chain: u64) -> u64 {
    EVM_SETTLED.with_borrow(|settled| settled.get(&(from, chain)).copied().unwrap_or(0))
}

/// When the timer next settles: `SETTLE_EVERY_NS` after it last started,
/// while a transfer needs it and no settling is under way.
pub fn settle_at() -> Option<u64> {
    if SETTLING.get() || storage::unsettled().is_empty() {
        return None;
    }
    Some(SETTLED_AT.get().saturating_add(SETTLE_EVERY_NS))
}

/// Puts the timer's next settling off to `SETTLE_EVERY_NS` after `now` when
/// it would otherwise run at once: a transfer that just ended its call needs
/// no second look straight away. A settling due sooner stays as it is.
fn settle_later(now: u64) {
    if !SETTLING.get() && SETTLED_AT.get().saturating_add(SETTLE_EVERY_NS) <= now {
        SETTLED_AT.set(now);
    }
}

/// Marks the timer's settling as under way until it ends, or until the code
/// after one of its calls traps: ic-cdk then drops the task, and with it
/// this guard.
struct Settling;

impl Drop for Settling {
    fn drop(&mut self) {
        SETTLING.set(false);
        schedule_timer(ic_cdk::api::time());
    }
}

/// Settles every transfer that needs it, as the wallet's timer does. The
/// ledger calls that may have run unseen are made again; the Bitcoin
/// transactions are watched account by account, with one read of each
/// account's outputs, and the EVM transactions account by account and chain
/// by chain, with one read of what the chain finalized.
pub async fn settle(now: u64) {
    if SETTLING.replace(true) {
        return;
    }
    SETTLED_AT.set(now);
    let _settling = Settling;
    let mut bitcoin: BTreeMap<AccountId, Vec<ActionId>> = BTreeMap::new();
    let mut evm: BTreeMap<(AccountId, u64), Vec<ActionId>> = BTreeMap::new();
    for id in storage::unsettled() {
        let Some(pending) = storage::action(id) else {
            continue;
        };
        let Action::Transfer(request) = &pending.action else {
            continue;
        };
        if pending.bitcoin_tx().is_some() {
            bitcoin.entry(request.from).or_default().push(id);
        } else if let Some(tx) = pending.evm_tx() {
            evm.entry((request.from, tx.chain_id)).or_default().push(id);
        } else {
            retry(id).await;
        }
    }
    for (from, ids) in bitcoin {
        watch(from, &ids).await;
    }
    for ((from, chain), ids) in evm {
        watch_evm(from, chain, &ids).await;
    }
}

/// Executes ledger transfer `id` again, as the wallet, when its last call
/// may have run unseen and none is in flight: the ledger finds the first by
/// its `created_at_time`, or sends it now.
async fn retry(id: ActionId) {
    let now = ic_cdk::api::time();
    let Some(mut pending) = storage::action(id) else {
        return;
    };
    let Action::Transfer(request) = pending.action.clone() else {
        return;
    };
    if pending.check_executable(id, now).is_err() {
        storage::set_action(id, pending);
        return;
    }
    if pending.transfer_retry(id, now) == Ok(true) {
        // How it ended is in the action.
        let _ = execute(id, pending, request, None, now).await;
    }
}

/// The unsettled Bitcoin transfers of account `from`.
fn watched_from(from: AccountId) -> Vec<ActionId> {
    storage::unsettled()
        .into_iter()
        .filter(|id| {
            storage::action(*id).is_some_and(|pending| {
                pending.bitcoin_tx().is_some()
                    && matches!(&pending.action, Action::Transfer(request) if request.from == from)
            })
        })
        .collect()
}

/// Watches the transactions of actions `ids`, all from Bitcoin account
/// `from`: reads the account's outputs once, records what they show, and
/// sends each transaction again whose turn it is.
async fn watch(from: AccountId, ids: &[ActionId]) {
    let Some(account) = storage::account(from) else {
        return;
    };
    let (AccountKind::Bitcoin(network), Ok(address)) = (account.kind(), account.bitcoin_address())
    else {
        return;
    };
    let Ok(utxos) = bitcoin_canister::coins(network, address.to_string()).await else {
        return;
    };
    let now = ic_cdk::api::time();
    let mut due = Vec::new();
    for id in ids {
        if observe(*id, from, &account, &utxos, now) == Next::Resend {
            due.push(*id);
        }
    }
    for id in due {
        resend(id, network).await;
    }
}

/// Records what `utxos`, the outputs of Bitcoin account `from`, show of the
/// transaction of action `id`, and says what to do next. A transaction
/// whose inputs stayed spent long enough settles: its inputs are released,
/// and a transfer still open ends `Executed`, since only its transaction
/// spends them.
fn observe(
    id: ActionId,
    from: AccountId,
    account: &StoredAccount,
    utxos: &Utxos,
    now: u64,
) -> Next {
    let Some(mut pending) = storage::action(id) else {
        return Next::Wait;
    };
    let Some(mut watch) = watched(&pending, account) else {
        return Next::Wait;
    };
    let next = watch.observe(utxos, now);
    if next == Next::Settled {
        let tx = pending
            .bitcoin_tx()
            .cloned()
            .expect("a watched transfer is built");
        let unsigned = btc::from_bytes(&tx.unsigned).expect("a stored transaction decodes");
        let asset = Chain::Bitcoin(match account.kind() {
            AccountKind::Bitcoin(network) => network,
            _ => unreachable!("a watched transaction is from a Bitcoin account"),
        });
        let tx_id = btc::txid(&unsigned);
        if pending.settled::<BitcoinTx>(from, asset.clone(), tx_id.clone(), now)
            && let Action::Transfer(request) = &pending.action
        {
            storage::add_transfer(StoredTransfer {
                account: from,
                chain: asset,
                to: request.to.clone(),
                amount: request.amount.clone(),
                tx_id,
                at_ns: now,
                action: id,
                fee: Some(Nat::from(tx.fee)),
            });
            log(None, AuditEvent::Executed(id), now);
        }
    } else {
        pending.bitcoin_observed(&watch);
    }
    storage::set_action(id, pending);
    next
}

/// The watch over the transaction of a transfer from `account`, while it
/// may have been sent and has not settled.
fn watched(pending: &StoredPending, account: &StoredAccount) -> Option<btc::Watched> {
    let tx = pending.bitcoin_tx()?;
    if !pending.may_have_sent() || tx.is_settled() {
        return None;
    }
    let own = account.bitcoin_address().ok()?.script_pubkey();
    // A stored transaction always decodes: the wallet encoded it.
    let unsigned = btc::from_bytes(&tx.unsigned).ok()?;
    Some(tx.watched(
        btc::txid_bytes(&unsigned),
        btc::spent_keys(&unsigned),
        btc::paid_to(&unsigned, &own),
    ))
}

/// Sends the transaction of action `id` again. A transfer still open is
/// executed again by the wallet, which sends the same transaction and ends
/// it `Executed` once the Bitcoin canister takes it.
async fn resend(id: ActionId, network: BitcoinNetwork) {
    let Some(signed) = resend_or_execute::<BitcoinTx>(id).await else {
        return;
    };
    // A send the Bitcoin canister refuses waits for its next turn.
    let _ = bitcoin_canister::send(network, signed).await;
}

/// The transaction of action `id` to send again, its send counted, when the
/// transfer is executed: one still open is executed again by the wallet,
/// which sends the same transaction and ends it `Executed` once it is
/// taken, and how that ended is in the action.
async fn resend_or_execute<T: Tx>(id: ActionId) -> Option<Vec<u8>> {
    let now = ic_cdk::api::time();
    let mut pending = storage::action(id)?;
    if matches!(pending.state, ActionState::Waiting | ActionState::Ready) {
        let Action::Transfer(request) = pending.action.clone() else {
            return None;
        };
        if pending.check_executable(id, now).is_ok() {
            let _ = execute(id, pending, request, None, now).await;
        }
        return None;
    }
    let signed = pending.tx::<T>()?.signed()?;
    // Counted before the call, so a send that never returns waits its turn
    // like any other.
    pending.resent::<T>(now);
    storage::set_action(id, pending);
    Some(signed)
}

/// Watches the transactions of actions `ids`, all from EVM account `from`
/// on `chain`, each as often as its looks allow: reads once how many of the
/// account's nonces the chain finalized. A transaction whose nonce it
/// finalized settles by its receipt; one whose nonce it did not is sent
/// again when its turn comes.
async fn watch_evm(from: AccountId, chain: u64, ids: &[ActionId]) {
    let now = ic_cdk::api::time();
    let due: Vec<ActionId> = ids
        .iter()
        .copied()
        .filter(|id| {
            storage::action(*id)
                .filter(|pending| pending.watches::<EvmTx>())
                .and_then(|pending| pending.evm_tx().map(|tx| tx.look_due(now)))
                .unwrap_or(false)
        })
        .collect();
    if due.is_empty() {
        return;
    }
    let Some(address) = storage::account(from).and_then(|account| account.evm_address().ok())
    else {
        return;
    };
    let Ok(finalized) = evm_rpc::transaction_count(chain, &address, BlockTag::Finalized).await
    else {
        return;
    };
    for id in due {
        let now = ic_cdk::api::time();
        let Some(mut pending) = storage::action(id).filter(|pending| pending.watches::<EvmTx>())
        else {
            continue;
        };
        pending.evm_looked(now);
        storage::set_action(id, pending.clone());
        let tx = pending
            .evm_tx()
            .cloned()
            .expect("a watched transaction is built");
        if tx.nonce < finalized {
            settle_evm(id, from, chain, &tx).await;
        } else if now >= tx.last_sent_ns.saturating_add(evm::resend_wait(tx.sends))
            && let Some(signed) = resend_or_execute::<EvmTx>(id).await
        {
            // A send the providers refuse waits for its next turn.
            let _ = evm_rpc::send(chain, &signed).await;
        }
    }
}

/// Settles the transaction `tx` of action `id`, from EVM account `from` on
/// `chain`, whose nonce the chain finalized, by its receipt: it ran, or it
/// reverted and paid its fees. A transfer still open ends `Executed` or
/// `Failed`; one recorded `Executed` when a provider took it stays so only
/// if it ran. The recorded transfer then holds what it sent and paid. Only
/// this transaction has its nonce, so providers with no receipt of it have
/// not caught up: the wallet looks again later.
async fn settle_evm(id: ActionId, from: AccountId, chain: u64, tx: &EvmTx) {
    let Some(signed) = &tx.signed else {
        return;
    };
    let hash = evm::hash_text(&evm::tx_hash(signed));
    let Ok(Some(receipt)) = evm_rpc::receipt(chain, &hash).await else {
        return;
    };
    let now = ic_cdk::api::time();
    let Some(mut pending) = storage::action(id).filter(|pending| pending.watches::<EvmTx>()) else {
        return;
    };
    let Action::Transfer(request) = pending.action.clone() else {
        return;
    };
    let asset = Chain::Evm(chain);
    let paid = receipt.gas_used.clone() * receipt.effective_gas_price.clone();
    let ran = receipt.status == Some(Nat::from(1u8));
    let recorded = if ran {
        if pending.settled::<EvmTx>(from, asset.clone(), hash.clone(), now) {
            log(None, AuditEvent::Executed(id), now);
            false
        } else {
            true
        }
    } else {
        let reason = format!(
            "transaction {hash} reverted on chain {chain}: it sent nothing and paid {} wei in fees",
            paid.0
        );
        let recorded = pending.settled_failed::<EvmTx>(reason);
        log(None, AuditEvent::Executed(id), now);
        recorded
    };
    EVM_SETTLED.with_borrow_mut(|settled| *settled.entry((from, chain)).or_default() += 1);
    let amount = if ran {
        request.amount.clone()
    } else {
        Nat::from(0u8)
    };
    let record = !recorded || !storage::settle_transfer(id, amount.clone(), paid.clone());
    if record && (ran || paid > 0u8) {
        storage::add_transfer(StoredTransfer {
            account: from,
            chain: asset,
            to: request.to,
            amount,
            tx_id: hash,
            at_ns: now,
            action: id,
            fee: Some(paid),
        });
    }
    storage::set_action(id, pending);
}

/// The balance of Bitcoin account `from` once the network confirms the
/// wallet's transactions from it (see `btc::available`). With none to
/// watch, the Bitcoin canister's balance; otherwise its outputs, read once,
/// which also tell how those transactions stand.
pub async fn bitcoin_balance(
    from: AccountId,
    account: &StoredAccount,
    network: BitcoinNetwork,
) -> Result<u64, WalletError> {
    let address = account.bitcoin_address()?.to_string();
    if watched_from(from).is_empty() {
        return bitcoin_canister::balance(network, address)
            .await
            .map_err(WalletError::ChainError);
    }
    let utxos = bitcoin_canister::coins(network, address)
        .await
        .map_err(WalletError::ChainError)?;
    let now = ic_cdk::api::time();
    let mut watching = Vec::new();
    for id in watched_from(from) {
        observe(id, from, account, &utxos, now);
        if let Some(watch) = storage::action(id)
            .as_ref()
            .and_then(|pending| watched(pending, account))
        {
            watching.push(watch);
        }
    }
    Ok(btc::available(&utxos.coins, &watching))
}
