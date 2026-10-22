//! A canister for the PocketIC tests that makes one call to another canister,
//! so a test can call a canister the way a canister does: without the ingress
//! check, `inspect_message`, that a message from outside the IC meets.

use candid::Principal;
use ic_cdk::call::{Call, CallFailed};
use serde_bytes::ByteBuf;

/// Calls `method` of `canister` with the Candid bytes `arg`, and returns the
/// reply's bytes, or the reject code and message.
#[ic_cdk::update]
async fn forward(
    canister: Principal,
    method: String,
    arg: ByteBuf,
) -> Result<ByteBuf, (u32, String)> {
    match Call::unbounded_wait(canister, &method)
        .with_raw_args(&arg)
        .await
    {
        Ok(reply) => Ok(ByteBuf::from(reply.into_bytes())),
        Err(CallFailed::CallRejected(rejected)) => Err((
            rejected.raw_reject_code(),
            rejected.reject_message().to_string(),
        )),
        Err(other) => Err((0, other.to_string())),
    }
}

/// Never replies: calls `spin_once` of `canister` again and again, so a call
/// it made is always in flight. Wallet code that calls a canister doing
/// this never lets the wallet stop.
#[ic_cdk::update]
async fn spin(canister: Principal) {
    loop {
        let _ = Call::unbounded_wait(canister, "spin_once").await;
    }
}

#[ic_cdk::update]
fn spin_once() {}

/// Gives `canister` every cycle it can spend but `keep`: wallet code that
/// empties the wallet before the owners repair it.
#[ic_cdk::update]
async fn drain(canister: Principal, keep: u128) -> Result<(), String> {
    let cycles = ic_cdk::api::canister_liquid_cycle_balance().saturating_sub(keep);
    Call::unbounded_wait(Principal::management_canister(), "deposit_cycles")
        .with_arg(CanisterIdRecord {
            canister_id: canister,
        })
        .with_cycles(cycles)
        .await
        .map(|_| ())
        .map_err(|failed| failed.to_string())
}

#[derive(candid::CandidType)]
struct CanisterIdRecord {
    canister_id: Principal,
}
