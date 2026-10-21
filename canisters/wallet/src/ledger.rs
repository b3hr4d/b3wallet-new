//! Calls to ICRC-1 ledgers. Every call waits a bounded time, so a ledger that
//! never answers cannot keep the wallet from stopping or upgrading.

use crate::transfers::{self, CallFailure};
use candid::{CandidType, Nat, Principal};
use ic_cdk::call::{Call, CallFailed};
use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use serde::de::DeserializeOwned;

async fn call<R: CandidType + DeserializeOwned>(
    ledger: Principal,
    method: &str,
    arg: impl CandidType,
) -> Result<R, CallFailure> {
    let response = Call::bounded_wait(ledger, method)
        .with_arg(arg)
        .await
        .map_err(|failed| match failed {
            CallFailed::CallRejected(rejected) => {
                let reason = format!(
                    "the ledger {ledger} rejected {method} ({}): {}",
                    rejected.raw_reject_code(),
                    rejected.reject_message()
                );
                transfers::rejected(rejected.reject_code().ok(), reason)
            }
            other => CallFailure::NotRun(format!("cannot call the ledger {ledger}: {other}")),
        })?;
    // The ledger ran the call, but what it answered cannot be read.
    response.candid().map_err(|error| {
        CallFailure::Unknown(format!(
            "the answer of the ledger {ledger} to {method} does not decode: {error}"
        ))
    })
}

/// The balance of `account`, from `icrc1_balance_of`.
pub async fn balance_of(ledger: Principal, account: LedgerAccount) -> Result<Nat, String> {
    call(ledger, "icrc1_balance_of", account)
        .await
        .map_err(|failure| match failure {
            CallFailure::NotRun(reason) | CallFailure::Unknown(reason) => reason,
        })
}

/// Sends `arg` with `icrc1_transfer`.
pub async fn transfer(
    ledger: Principal,
    arg: TransferArg,
) -> Result<Result<Nat, TransferError>, CallFailure> {
    call(ledger, "icrc1_transfer", arg).await
}
