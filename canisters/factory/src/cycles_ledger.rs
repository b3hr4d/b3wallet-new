//! The cycles ledger creates each wallet with `create_canister_from`: it
//! takes the cycles out of the creator's account, under the allowance the
//! creator gave the factory, and has the cycles minting canister create the
//! canister with them. The factory never holds a creator's cycles, and the
//! ledger refunds them if the creation fails.

use crate::calls::{Wait, call};
use crate::types::FactoryError;
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// The cycles ledger's id, the same on mainnet, in PocketIC and on a local
/// icp-cli network.
pub fn id() -> Principal {
    // um5iw-rqaaa-aaaaq-qaaba-cai
    Principal::from_slice(&[0, 0, 0, 0, 2, 16, 0, 2, 1, 1])
}

const WHO: &str = "the cycles ledger";

/// The cycles ledger fills in a field left out: the other settings are the
/// IC's defaults.
#[derive(CandidType)]
struct CanisterSettings {
    controllers: Option<Vec<Principal>>,
}

/// The cycles minting canister's `SubnetSelection`, the one arm the factory
/// sends.
#[derive(CandidType)]
enum SubnetSelection {
    Subnet { subnet: Principal },
}

#[derive(CandidType)]
struct CmcCreateCanisterArgs {
    settings: Option<CanisterSettings>,
    /// None: the cycles minting canister chooses one of its default subnets.
    subnet_selection: Option<SubnetSelection>,
}

#[derive(CandidType)]
struct CreateCanisterFromArgs {
    from: Account,
    spender_subaccount: Option<ByteBuf>,
    created_at_time: Option<u64>,
    amount: Nat,
    creation_args: Option<CmcCreateCanisterArgs>,
}

#[derive(CandidType, Deserialize)]
struct CreateCanisterSuccess {
    canister_id: Principal,
}

#[derive(CandidType, Deserialize, Debug)]
enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

#[derive(CandidType, Deserialize, Debug)]
enum CreateCanisterFromError {
    InsufficientFunds {
        balance: Nat,
    },
    InsufficientAllowance {
        allowance: Nat,
    },
    TooOld,
    CreatedInFuture {
        ledger_time: u64,
    },
    TemporarilyUnavailable,
    Duplicate {
        duplicate_of: Nat,
        canister_id: Option<Principal>,
    },
    FailedToCreateFrom {
        create_from_block: Option<Nat>,
        refund_block: Option<Nat>,
        approval_refund_block: Option<Nat>,
        rejection_code: RejectionCode,
        rejection_reason: String,
    },
    GenericError {
        message: String,
        error_code: Nat,
    },
}

/// Creates a canister controlled by the factory alone, with `cycles` out of
/// `from`, on `subnet` or, for None, a subnet the cycles minting canister
/// chooses. The ledger also takes its fee from `from`, and the allowance
/// must cover both.
pub async fn create_canister_from(
    from: Account,
    cycles: u128,
    subnet: Option<Principal>,
) -> Result<Principal, FactoryError> {
    let args = CreateCanisterFromArgs {
        from,
        spender_subaccount: None,
        created_at_time: None,
        amount: Nat::from(cycles),
        creation_args: Some(CmcCreateCanisterArgs {
            settings: Some(CanisterSettings {
                controllers: Some(vec![ic_cdk::api::canister_self()]),
            }),
            subnet_selection: subnet.map(|subnet| SubnetSelection::Subnet { subnet }),
        }),
    };
    let result: Result<CreateCanisterSuccess, CreateCanisterFromError> =
        call(id(), WHO, "create_canister_from", (args,), Wait::Unbounded)
            .await
            .map_err(FactoryError::CreateFailed)?;
    match result {
        Ok(success) => Ok(success.canister_id),
        Err(CreateCanisterFromError::InsufficientAllowance { allowance }) => {
            Err(FactoryError::InsufficientAllowance { allowance })
        }
        Err(CreateCanisterFromError::InsufficientFunds { balance }) => {
            Err(FactoryError::InsufficientFunds { balance })
        }
        Err(other) => Err(FactoryError::CreateFailed(format!(
            "{WHO} did not create the canister: {other:?}"
        ))),
    }
}
