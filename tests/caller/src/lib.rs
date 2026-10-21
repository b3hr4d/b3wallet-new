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
