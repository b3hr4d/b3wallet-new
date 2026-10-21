//! Calls to ICRC-1 ledgers.

use crate::calls::{self, call};
use crate::transfers::CallFailure;
use candid::{Nat, Principal};
use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};

/// The balance of `account`, from `icrc1_balance_of`.
pub async fn balance_of(ledger: Principal, account: LedgerAccount) -> Result<Nat, String> {
    call(ledger, who(ledger), "icrc1_balance_of", account, 0)
        .await
        .map_err(calls::reason)
}

/// Sends `arg` with `icrc1_transfer`.
pub async fn transfer(
    ledger: Principal,
    arg: TransferArg,
) -> Result<Result<Nat, TransferError>, CallFailure> {
    call(ledger, who(ledger), "icrc1_transfer", arg, 0).await
}

fn who(ledger: Principal) -> String {
    format!("the ledger {ledger}")
}
