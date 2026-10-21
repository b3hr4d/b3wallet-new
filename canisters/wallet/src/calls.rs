//! Calls to other canisters. Every call waits a bounded time, so a canister
//! that never answers cannot keep the wallet from stopping or upgrading.
//!
//! The wallet counts the calls whose answer it still waits for, and starts
//! none while it installs an upgrade: an answer that reached the new version
//! would run a callback whose state went with the old heap.

use crate::storage;
use crate::transfers::{self, CallFailure};
use candid::{CandidType, Principal};
use ic_cdk::call::{Call, CallFailed};
use serde::de::DeserializeOwned;
use std::cell::Cell;
use std::fmt::Display;

thread_local! {
    /// The calls started and not yet answered. On the heap: a new version
    /// starts with none, since none may be waiting when it is installed.
    static IN_FLIGHT: Cell<u32> = const { Cell::new(0) };
}

/// How many calls wait for their answer.
pub fn in_flight() -> u32 {
    IN_FLIGHT.get()
}

/// Counts one call from its start until its answer, or until its callback
/// traps: ic-cdk then drops the call's future, and with it this guard, and
/// keeps what the guard writes.
struct Waiting;

impl Waiting {
    fn start() -> Waiting {
        IN_FLIGHT.set(IN_FLIGHT.get() + 1);
        Waiting
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        IN_FLIGHT.set(IN_FLIGHT.get().saturating_sub(1));
    }
}

/// Calls `method` of `canister`, named `who` in errors, with `cycles`
/// attached.
pub async fn call<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
    arg: impl CandidType,
    cycles: u128,
) -> Result<R, CallFailure> {
    let arg = candid::encode_one(arg).expect("a call argument encodes");
    send(canister, who, method, arg, cycles).await
}

/// Calls `method` of `canister`, which takes no arguments.
pub async fn call_without_args<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
) -> Result<R, CallFailure> {
    let args = candid::encode_args(()).expect("no arguments encode");
    send(canister, who, method, args, 0).await
}

async fn send<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
    args: Vec<u8>,
    cycles: u128,
) -> Result<R, CallFailure> {
    if storage::upgrade().is_installing() {
        return Err(CallFailure::NotRun(format!(
            "the wallet does not call {who} while it installs an upgrade"
        )));
    }
    let waiting = Waiting::start();
    let response = Call::bounded_wait(canister, method)
        .with_raw_args(&args)
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
        });
    drop(waiting);
    let response = response?;
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
