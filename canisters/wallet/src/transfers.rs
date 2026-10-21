//! Transfers of ICP and ICRC tokens: what `propose` checks in a `Transfer`,
//! the ledger call `execute` makes, and how it reads the ledger's answer.
//! Pure: `ledger` makes the call and `actions` records how it ended.
//!
//! A ledger finds a repeated transfer by its `created_at_time` for 24 hours
//! and answers it with the block of the first. So when the wallet cannot tell
//! whether an attempt reached the ledger, it retries with the same
//! `created_at_time`, and the ledger sends the tokens at most once.

use crate::accounts::{self, StoredAccount};
use crate::policy::{DAY_NS, HOUR_NS};
use crate::{TransferRequest, WalletError};
use candid::{Nat, Principal};
use ic_cdk::call::RejectCode;
use icrc_ledger_types::icrc1::account::Account as LedgerAccount;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use std::str::FromStr;

/// The longest memo, in bytes: the ICRC-1 ledgers' default.
pub const MAX_MEMO_BYTES: usize = 32;
/// How long a ledger finds a repeated transfer: the transaction window of
/// the ICP and ICRC-1 ledgers.
pub const DEDUP_WINDOW_NS: u64 = DAY_NS;
/// A call still marked in flight after this long never returned to the
/// wallet's code, since a bounded-wait call answers within 300 seconds.
pub const STALE_CALL_NS: u64 = HOUR_NS;

/// Checks a transfer from `account`, the stored account `request.from`
/// names, and returns the ledger account it goes to.
pub fn check(
    request: &TransferRequest,
    account: Option<&StoredAccount>,
) -> Result<LedgerAccount, WalletError> {
    let Some(account) = account else {
        return Err(WalletError::NotFound(format!("account {}", request.from)));
    };
    accounts::ledger(&account.chain)?;
    let to = LedgerAccount::from_str(&request.to).map_err(|error| {
        WalletError::InvalidArgument(format!("`to` is not an ICRC-1 account: {error}"))
    })?;
    if to.owner == Principal::anonymous() {
        return Err(WalletError::InvalidArgument(
            "the anonymous principal cannot be sent tokens".to_string(),
        ));
    }
    if request.amount == 0u8 {
        return Err(WalletError::InvalidArgument(
            "a transfer needs an amount above 0".to_string(),
        ));
    }
    if request
        .memo
        .as_ref()
        .is_some_and(|memo| memo.len() > MAX_MEMO_BYTES)
    {
        return Err(WalletError::InvalidArgument(format!(
            "a memo has at most {MAX_MEMO_BYTES} bytes"
        )));
    }
    Ok(to)
}

/// The `icrc1_transfer` argument of an attempt created at `created_at_ns`.
pub fn ledger_arg(
    request: &TransferRequest,
    account: &StoredAccount,
    to: LedgerAccount,
    created_at_ns: u64,
) -> TransferArg {
    TransferArg {
        from_subaccount: accounts::subaccount(account.index),
        to,
        fee: request.fee.clone(),
        created_at_time: Some(created_at_ns),
        memo: request.memo.clone().map(Memo::from),
        amount: request.amount.clone(),
    }
}

/// How a call to the ledger failed before it could answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallFailure {
    /// The ledger did not run the call: it was never sent, or it was
    /// rejected or trapped, which rolls the ledger back.
    NotRun(String),
    /// The call may or may not have run: it timed out, or the answer did not
    /// decode.
    Unknown(String),
}

/// How a ledger call the IC rejected failed, by its reject code (`None` for
/// a code the wallet does not know). Only a reject by the system before the
/// ledger ran the call proves it did not run. A ledger that committed a
/// transfer and then trapped after an await of its own rejects with
/// `CanisterError`, and one that rejects explicitly may also do so after
/// committing, so those, like a bounded wait that ran out, leave it open.
pub fn rejected(code: Option<RejectCode>, reason: String) -> CallFailure {
    match code {
        Some(RejectCode::SysFatal | RejectCode::SysTransient | RejectCode::DestinationInvalid) => {
            CallFailure::NotRun(reason)
        }
        Some(RejectCode::CanisterReject | RejectCode::CanisterError | RejectCode::SysUnknown)
        | None => CallFailure::Unknown(reason),
    }
}

/// How an attempt ended, as far as the wallet can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The ledger holds the transfer in this block, sent now or by an
    /// earlier attempt with the same `created_at_time`.
    Sent(Nat),
    /// The ledger refused the transfer, so this attempt sent nothing.
    /// Trying again would not help. A ledger may refuse before it looks for
    /// an earlier attempt, as a changed fee is, so this tells nothing of
    /// one.
    Refused(String),
    /// This attempt did not run; a later one may.
    NotRun(String),
    /// This attempt may or may not have run.
    Unknown(String),
    /// The ledger no longer finds transfers with this `created_at_time`.
    TooOld,
}

fn digits(amount: &Nat) -> String {
    amount.0.to_str_radix(10)
}

/// Reads the result of an `icrc1_transfer` call.
pub fn read_reply(result: Result<Result<Nat, TransferError>, CallFailure>) -> Reply {
    match result {
        Ok(Ok(block)) => Reply::Sent(block),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => Reply::Sent(duplicate_of),
        Ok(Err(TransferError::BadFee { expected_fee })) => {
            Reply::Refused(format!("the ledger's fee is {}", digits(&expected_fee)))
        }
        Ok(Err(TransferError::BadBurn { min_burn_amount })) => Reply::Refused(format!(
            "the ledger burns no less than {}",
            digits(&min_burn_amount)
        )),
        Ok(Err(TransferError::InsufficientFunds { balance })) => Reply::Refused(format!(
            "the account holds {}, less than the amount and the fee",
            digits(&balance)
        )),
        Ok(Err(TransferError::GenericError {
            error_code,
            message,
        })) => Reply::Refused(format!(
            "the ledger refused it: {message} (error {})",
            digits(&error_code)
        )),
        Ok(Err(TransferError::TooOld)) => Reply::TooOld,
        Ok(Err(TransferError::CreatedInFuture { ledger_time })) => Reply::NotRun(format!(
            "the ledger's clock ({ledger_time} ns) is behind the wallet's"
        )),
        Ok(Err(TransferError::TemporarilyUnavailable)) => {
            Reply::NotRun("the ledger is temporarily unavailable".to_string())
        }
        Err(CallFailure::NotRun(reason)) => Reply::NotRun(reason),
        Err(CallFailure::Unknown(reason)) => Reply::Unknown(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountHolder, Chain};
    use serde_bytes::ByteBuf;

    fn account(index: u32) -> StoredAccount {
        StoredAccount {
            chain: Chain::Icp,
            label: "main".to_string(),
            index,
            holder: AccountHolder::Wallet,
            added_at_ns: 0,
            added_by: Principal::self_authenticating([1; 32]),
        }
    }

    fn to() -> LedgerAccount {
        LedgerAccount::from(Principal::self_authenticating([2; 32]))
    }

    fn request(to: &str, amount: u64, memo: Option<Vec<u8>>) -> TransferRequest {
        TransferRequest {
            from: 1,
            to: to.to_string(),
            amount: Nat::from(amount),
            fee: None,
            memo: memo.map(ByteBuf::from),
        }
    }

    #[test]
    fn a_transfer_needs_an_account_a_recipient_an_amount_and_a_short_memo() {
        let to_text = to().to_string();
        let good = request(&to_text, 5, Some(vec![7; MAX_MEMO_BYTES]));
        assert_eq!(check(&good, Some(&account(0))), Ok(to()));
        assert!(matches!(
            check(&good, None),
            Err(WalletError::NotFound(reason)) if reason == "account 1"
        ));

        let anonymous = Principal::anonymous().to_text();
        let refused = [
            request("not an account", 5, None),
            request(&format!("{to_text}.1"), 5, None),
            request(&anonymous, 5, None),
            request(&to_text, 0, None),
            request(&to_text, 5, Some(vec![7; MAX_MEMO_BYTES + 1])),
        ];
        for request in refused {
            assert!(
                matches!(
                    check(&request, Some(&account(0))),
                    Err(WalletError::InvalidArgument(_))
                ),
                "{request:?}"
            );
        }

        let bitcoin = StoredAccount {
            chain: Chain::Bitcoin(crate::BitcoinNetwork::Regtest),
            ..account(0)
        };
        assert!(matches!(
            check(&good, Some(&bitcoin)),
            Err(WalletError::InvalidArgument(reason)) if reason.contains("cannot hold")
        ));
    }

    #[test]
    fn the_ledger_call_sends_from_the_account_index() {
        let mut request = request(&to().to_string(), 5, Some(vec![1, 2]));
        request.fee = Some(Nat::from(10u8));
        let arg = ledger_arg(&request, &account(3), to(), 42);
        assert_eq!(arg.from_subaccount, accounts::subaccount(3));
        assert_eq!(arg.to, to());
        assert_eq!(arg.amount, Nat::from(5u8));
        assert_eq!(arg.fee, Some(Nat::from(10u8)));
        assert_eq!(arg.created_at_time, Some(42));
        assert_eq!(arg.memo, Some(Memo::from(vec![1, 2])));
        assert_eq!(
            ledger_arg(&request, &account(0), to(), 42).from_subaccount,
            None
        );
    }

    #[test]
    fn a_duplicate_is_the_earlier_block_and_a_lost_answer_is_unknown() {
        let block = Nat::from(1_234_567u32);
        assert_eq!(
            read_reply(Ok(Ok(block.clone()))),
            Reply::Sent(block.clone())
        );
        assert_eq!(
            read_reply(Ok(Err(TransferError::Duplicate {
                duplicate_of: block.clone()
            }))),
            Reply::Sent(block)
        );
        assert_eq!(
            read_reply(Ok(Err(TransferError::InsufficientFunds {
                balance: Nat::from(1_000_000u32)
            }))),
            Reply::Refused(
                "the account holds 1000000, less than the amount and the fee".to_string()
            )
        );
        for refused in [
            TransferError::BadFee {
                expected_fee: Nat::from(10u8),
            },
            TransferError::BadBurn {
                min_burn_amount: Nat::from(10u8),
            },
            TransferError::GenericError {
                error_code: Nat::from(1u8),
                message: "no".to_string(),
            },
        ] {
            assert!(matches!(read_reply(Ok(Err(refused))), Reply::Refused(_)));
        }
        for not_run in [
            TransferError::TemporarilyUnavailable,
            TransferError::CreatedInFuture { ledger_time: 1 },
        ] {
            assert!(matches!(read_reply(Ok(Err(not_run))), Reply::NotRun(_)));
        }
        assert_eq!(read_reply(Ok(Err(TransferError::TooOld))), Reply::TooOld);
        assert_eq!(
            read_reply(Err(CallFailure::NotRun("stopped".to_string()))),
            Reply::NotRun("stopped".to_string())
        );
        assert_eq!(
            read_reply(Err(CallFailure::Unknown("timeout".to_string()))),
            Reply::Unknown("timeout".to_string())
        );
    }

    #[test]
    fn only_a_reject_by_the_system_proves_a_call_did_not_run() {
        for code in [
            RejectCode::SysFatal,
            RejectCode::SysTransient,
            RejectCode::DestinationInvalid,
        ] {
            assert_eq!(
                rejected(Some(code), "r".to_string()),
                CallFailure::NotRun("r".to_string())
            );
        }
        // The ledger may have committed the transfer before it trapped or
        // rejected; a bounded wait may have run out after it ran.
        for code in [
            Some(RejectCode::CanisterReject),
            Some(RejectCode::CanisterError),
            Some(RejectCode::SysUnknown),
            None,
        ] {
            assert_eq!(
                rejected(code, "r".to_string()),
                CallFailure::Unknown("r".to_string())
            );
        }
    }
}
