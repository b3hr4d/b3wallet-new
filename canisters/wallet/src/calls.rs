//! Calls to other canisters. Every call waits a bounded time, so a canister
//! that never answers cannot keep the wallet from stopping or upgrading.

use crate::transfers::{self, CallFailure};
use candid::{CandidType, Principal};
use ic_cdk::call::{Call, CallFailed};
use serde::de::DeserializeOwned;
use std::fmt::Display;

/// Calls `method` of `canister`, named `who` in errors, with `cycles`
/// attached.
pub async fn call<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
    arg: impl CandidType,
    cycles: u128,
) -> Result<R, CallFailure> {
    let response = Call::bounded_wait(canister, method)
        .with_arg(arg)
        .with_cycles(cycles)
        .await
        .map_err(|failed| match failed {
            CallFailed::CallRejected(rejected) => {
                let reason = format!(
                    "{who} rejected {method} ({}): {}",
                    rejected.raw_reject_code(),
                    rejected.reject_message()
                );
                transfers::rejected(rejected.reject_code().ok(), reason)
            }
            other => CallFailure::NotRun(format!("cannot call {who}: {other}")),
        })?;
    // The canister ran the call, but what it answered cannot be read.
    response.candid().map_err(|error| {
        CallFailure::Unknown(format!(
            "the answer of {who} to {method} does not decode: {error}"
        ))
    })
}

/// The reason of a failed call whose outcome changes nothing: a read.
pub fn reason(failure: CallFailure) -> String {
    match failure {
        CallFailure::NotRun(reason) | CallFailure::Unknown(reason) => reason,
    }
}
