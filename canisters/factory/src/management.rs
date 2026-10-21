//! Calls to the management canister about a canister the factory created:
//! its chunk store, the install, its module, and its controllers.

use crate::calls::{Wait, call};
use candid::{CandidType, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

const WHO: &str = "the management canister";

fn management() -> Principal {
    Principal::management_canister()
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
enum InstallMode {
    #[serde(rename = "install")]
    Install,
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

/// The IC leaves every setting left out as it is.
#[derive(CandidType)]
struct Settings {
    controllers: Option<Vec<Principal>>,
}

#[derive(CandidType)]
struct UpdateSettingsArgs {
    canister_id: Principal,
    settings: Settings,
    sender_canister_version: Option<u64>,
}

#[derive(CandidType)]
struct CanisterInfoArgs {
    canister_id: Principal,
    num_requested_changes: Option<u64>,
}

/// The fields of `canister_info`'s answer the factory reads.
#[derive(CandidType, Deserialize)]
pub struct CanisterInfo {
    pub module_hash: Option<ByteBuf>,
    pub controllers: Vec<Principal>,
}

/// Adds a chunk to `canister`'s chunk store and returns the hash it is
/// stored under.
pub async fn upload_chunk(canister: Principal, chunk: Vec<u8>) -> Result<Vec<u8>, String> {
    let arg = UploadChunkArgs {
        canister_id: canister,
        chunk: ByteBuf::from(chunk),
    };
    let hash: ChunkHash = call(management(), WHO, "upload_chunk", (arg,), Wait::Unbounded).await?;
    Ok(hash.hash.into_vec())
}

/// Installs, in the empty `canister`, the module whose chunks its chunk
/// store holds, with the install argument `arg`.
pub async fn install_chunked_code(
    canister: Principal,
    chunk_hashes: Vec<ByteBuf>,
    wasm_module_hash: ByteBuf,
    arg: Vec<u8>,
) -> Result<(), String> {
    let arg = InstallChunkedCodeArgs {
        mode: InstallMode::Install,
        target_canister: canister,
        store_canister: None,
        chunk_hashes_list: chunk_hashes
            .into_iter()
            .map(|hash| ChunkHash { hash })
            .collect(),
        wasm_module_hash,
        arg: ByteBuf::from(arg),
        sender_canister_version: Some(ic_cdk::api::canister_version()),
    };
    call(
        management(),
        WHO,
        "install_chunked_code",
        (arg,),
        Wait::Unbounded,
    )
    .await
}

pub async fn clear_chunk_store(canister: Principal) -> Result<(), String> {
    let arg = CanisterIdRecord {
        canister_id: canister,
    };
    call(
        management(),
        WHO,
        "clear_chunk_store",
        (arg,),
        Wait::Unbounded,
    )
    .await
}

/// The module `canister` runs and its controllers. Any canister may ask, so
/// the factory learns whether it still controls a canister.
pub async fn info(canister: Principal) -> Result<CanisterInfo, String> {
    let arg = CanisterInfoArgs {
        canister_id: canister,
        num_requested_changes: None,
    };
    call(management(), WHO, "canister_info", (arg,), Wait::Unbounded).await
}

/// Makes `controller` the only controller of `canister`, which drops the
/// factory.
pub async fn hand_over(canister: Principal, controller: Principal) -> Result<(), String> {
    let arg = UpdateSettingsArgs {
        canister_id: canister,
        settings: Settings {
            controllers: Some(vec![controller]),
        },
        sender_canister_version: Some(ic_cdk::api::canister_version()),
    };
    call(
        management(),
        WHO,
        "update_settings",
        (arg,),
        Wait::Unbounded,
    )
    .await
}
