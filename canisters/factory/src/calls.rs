//! Calls to other canisters. Reads of the release channel wait a bounded
//! time, since it is just another canister. The cycles ledger and the
//! management canister are the IC's own, and the factory waits for their
//! answer, so it always learns whether a canister was created and paid for,
//! and whether a release installed.

use candid::utils::ArgumentEncoder;
use candid::{CandidType, Principal};
use ic_cdk::call::{Call, CallFailed};
use serde::de::DeserializeOwned;
use std::fmt::Display;

/// How long the factory waits for the answer.
#[derive(Clone, Copy)]
pub enum Wait {
    Bounded,
    Unbounded,
}

/// Calls `method` of `canister`, named `who` in errors, with the Candid
/// arguments `args`.
pub async fn call<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
    args: impl ArgumentEncoder,
    wait: Wait,
) -> Result<R, String> {
    let call = match wait {
        Wait::Bounded => Call::bounded_wait(canister, method),
        Wait::Unbounded => Call::unbounded_wait(canister, method),
    };
    let response = call.with_args(&args).await.map_err(|failed| match failed {
        CallFailed::CallRejected(rejected) => format!(
            "{who} rejected {method} ({}): {}",
            rejected.raw_reject_code(),
            rejected.reject_message()
        ),
        other => format!("cannot call {who}: {other}"),
    })?;
    response
        .candid()
        .map_err(|error| format!("the answer of {who} to {method} does not decode: {error}"))
}
