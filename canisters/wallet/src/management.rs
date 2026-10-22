//! Calls to the management canister about the wallet itself: the module it
//! runs, its chunk store, and installing a release from it. The install
//! needs the wallet to be its own controller.

use crate::calls::{self, call};
use candid::{CandidType, Principal};
use ic_cdk::call::Call;
use serde::Deserialize;
use serde_bytes::ByteBuf;

const WHO: &str = "the management canister";

#[derive(CandidType)]
struct CanisterInfoArgs {
    canister_id: Principal,
    num_requested_changes: Option<u64>,
}

/// The fields of `canister_info`'s answer the wallet reads.
#[derive(CandidType, Deserialize)]
struct CanisterInfo {
    module_hash: Option<ByteBuf>,
    controllers: Vec<Principal>,
}

/// What the IC says about the wallet's own canister.
pub struct Itself {
    /// The SHA-256 of the module the wallet runs.
    pub module_hash: Option<Vec<u8>>,
    /// Whether the wallet is one of its own controllers, which the install
    /// needs.
    pub controls_itself: bool,
}

#[derive(CandidType)]
struct CanisterIdRecord {
    canister_id: Principal,
}

#[derive(CandidType)]
struct UploadChunkArgs {
    canister_id: Principal,
    chunk: ByteBuf,
}

#[derive(CandidType, Deserialize)]
struct ChunkHash {
    hash: ByteBuf,
}

#[derive(CandidType, Deserialize)]
struct UpgradeFlags {
    skip_pre_upgrade: Option<bool>,
}

#[derive(CandidType, Deserialize)]
enum InstallMode {
    #[serde(rename = "upgrade")]
    Upgrade(Option<UpgradeFlags>),
}

#[derive(CandidType)]
struct InstallChunkedCodeArgs {
    mode: InstallMode,
    target_canister: Principal,
    store_canister: Option<Principal>,
    chunk_hashes_list: Vec<ChunkHash>,
    wasm_module_hash: ByteBuf,
    arg: ByteBuf,
    sender_canister_version: Option<u64>,
}

fn wallet() -> Principal {
    ic_cdk::api::canister_self()
}

/// The module the wallet runs and whether it controls itself.
pub async fn itself() -> Result<Itself, String> {
    let arg = CanisterInfoArgs {
        canister_id: wallet(),
        num_requested_changes: None,
    };
    let info: CanisterInfo = call(
        Principal::management_canister(),
        WHO,
        "canister_info",
        arg,
        0,
    )
    .await
    .map_err(calls::reason)?;
    Ok(Itself {
        module_hash: info.module_hash.map(ByteBuf::into_vec),
        controls_itself: info.controllers.contains(&wallet()),
    })
}

/// Empties the chunk store.
pub async fn clear_chunk_store() -> Result<(), String> {
    let arg = CanisterIdRecord {
        canister_id: wallet(),
    };
    call(
        Principal::management_canister(),
        WHO,
        "clear_chunk_store",
        arg,
        0,
    )
    .await
    .map_err(calls::reason)
}

/// Empties the chunk store without waiting for the answer, so no reply
/// can outlive the version that sent it.
pub fn clear_chunk_store_oneway() -> Result<(), String> {
    Call::unbounded_wait(Principal::management_canister(), "clear_chunk_store")
        .with_arg(CanisterIdRecord {
            canister_id: wallet(),
        })
        .oneway()
        .map_err(|error| format!("cannot call {WHO}: {error}"))
}

/// Adds a chunk to the chunk store and returns the hash it is stored under.
pub async fn upload_chunk(chunk: Vec<u8>) -> Result<Vec<u8>, String> {
    let arg = UploadChunkArgs {
        canister_id: wallet(),
        chunk: ByteBuf::from(chunk),
    };
    let hash: ChunkHash = call(
        Principal::management_canister(),
        WHO,
        "upload_chunk",
        arg,
        0,
    )
    .await
    .map_err(calls::reason)?;
    Ok(hash.hash.into_vec())
}

/// Sends the wallet's upgrade to the module whose chunks the chunk store
/// holds, one way: the reply would reach a callback of the old version in
/// the new one.
pub fn install(chunk_hashes: Vec<ByteBuf>, wasm_module_hash: ByteBuf) -> Result<(), String> {
    let arg = InstallChunkedCodeArgs {
        mode: InstallMode::Upgrade(None),
        target_canister: wallet(),
        store_canister: None,
        chunk_hashes_list: chunk_hashes
            .into_iter()
            .map(|hash| ChunkHash { hash })
            .collect(),
        wasm_module_hash,
        arg: ByteBuf::from(
            candid::encode_one(crate::WalletArg::Upgrade(None)).expect("WalletArg encodes"),
        ),
        sender_canister_version: Some(ic_cdk::api::canister_version()),
    };
    Call::unbounded_wait(Principal::management_canister(), "install_chunked_code")
        .with_arg(arg)
        .oneway()
        .map_err(|error| format!("cannot call {WHO}: {error}"))
}
