//! Calls to the management canister about the wallet and the controller
//! itself, and the calls to another canister: asking a new controller what
//! it serves and how it is set up.
//!
//! The management canister is the IC's own, and the controller waits for its
//! answer, so it always learns whether the wallet stopped, a chunk was
//! stored, or an install or a change of controllers happened. The controller
//! is never upgraded, so no answer can reach other code than the code that
//! asked.

use candid::utils::ArgumentEncoder;
use candid::{CandidType, Nat, Principal};
use ic_cdk::call::{Call, CallFailed};
use recovery::InstallMode;
use recovery::controller::Health;
use recovery::history::CanisterInfo;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_bytes::ByteBuf;
use std::fmt::Display;

const WHO: &str = "the management canister";

fn management() -> Principal {
    Principal::management_canister()
}

#[derive(Clone, Copy)]
enum Wait {
    Bounded,
    Unbounded,
}

async fn call<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
    args: impl ArgumentEncoder,
    wait: Wait,
) -> Result<R, String> {
    call_with_cycles(canister, who, method, args, wait, 0).await
}

async fn call_with_cycles<R: CandidType + DeserializeOwned>(
    canister: Principal,
    who: impl Display,
    method: &str,
    args: impl ArgumentEncoder,
    wait: Wait,
    cycles: u128,
) -> Result<R, String> {
    let call = match wait {
        Wait::Bounded => Call::bounded_wait(canister, method),
        Wait::Unbounded => Call::unbounded_wait(canister, method),
    };
    let call = call.with_args(&args).with_cycles(cycles);
    let response = call.await.map_err(|failed| match failed {
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

#[derive(CandidType)]
struct CanisterIdRecord {
    canister_id: Principal,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopping")]
    Stopping,
    #[serde(rename = "stopped")]
    Stopped,
}

#[derive(CandidType, Deserialize)]
struct DefiniteSettings {
    controllers: Vec<Principal>,
    freezing_threshold: Nat,
    reserved_cycles_limit: Nat,
    wasm_memory_limit: Nat,
    memory_allocation: Nat,
    compute_allocation: Nat,
}

/// The fields of `canister_status`'s answer the controller reads.
#[derive(CandidType, Deserialize)]
struct CanisterStatus {
    status: RunStatus,
    settings: DefiniteSettings,
    module_hash: Option<ByteBuf>,
    cycles: Nat,
    idle_cycles_burned_per_day: Nat,
}

async fn canister_status(canister: Principal) -> Result<CanisterStatus, String> {
    call(
        management(),
        WHO,
        "canister_status",
        (CanisterIdRecord {
            canister_id: canister,
        },),
        Wait::Unbounded,
    )
    .await
}

/// What the IC says about this controller's settings, which a canister may
/// ask about itself, and its subnet.
pub async fn own_health() -> Result<Health, String> {
    let status = canister_status(ic_cdk::api::canister_self()).await?;
    let settings = status.settings;
    Ok(Health {
        subnet: ic_cdk::api::subnet_self(),
        controllers: settings.controllers,
        freezing_threshold: settings.freezing_threshold,
        reserved_cycles_limit: settings.reserved_cycles_limit,
        wasm_memory_limit: settings.wasm_memory_limit,
        memory_allocation: settings.memory_allocation,
        compute_allocation: settings.compute_allocation,
        cycles: status.cycles,
        idle_cycles_burned_per_day: status.idle_cycles_burned_per_day,
    })
}

/// What `canister_status` says about the wallet. Only a controller may ask,
/// so an answer also proves the controller controls the wallet.
pub struct WalletStatus {
    pub status: RunStatus,
    pub module_hash: Option<Vec<u8>>,
    /// The cycles the wallet can spend above what its freezing threshold
    /// keeps.
    pub liquid_cycles: u128,
}

pub async fn wallet_status(wallet: Principal) -> Result<WalletStatus, String> {
    let status = canister_status(wallet).await?;
    let cycles = to_u128(&status.cycles);
    let reserve = to_u128(&status.idle_cycles_burned_per_day)
        .saturating_mul(to_u128(&status.settings.freezing_threshold))
        / 86_400;
    Ok(WalletStatus {
        status: status.status,
        module_hash: status.module_hash.map(ByteBuf::into_vec),
        liquid_cycles: cycles.saturating_sub(reserve),
    })
}

fn to_u128(value: &Nat) -> u128 {
    u128::try_from(value.0.clone()).unwrap_or(u128::MAX)
}

#[derive(CandidType)]
struct CanisterInfoArgs {
    canister_id: Principal,
    num_requested_changes: Option<u64>,
}

/// The module `canister` runs, its controllers, how many changes of its
/// code, controllers or snapshots the IC has recorded, and the last
/// `changes` of them. Any canister may ask.
pub async fn info(canister: Principal, changes: u64) -> Result<CanisterInfo, String> {
    call(
        management(),
        WHO,
        "canister_info",
        (CanisterInfoArgs {
            canister_id: canister,
            num_requested_changes: Some(changes),
        },),
        Wait::Unbounded,
    )
    .await
}

/// Stops the wallet. The IC answers once every call the wallet made has
/// been answered, or with an error after its own timeout.
pub async fn stop(wallet: Principal) -> Result<(), String> {
    call(
        management(),
        WHO,
        "stop_canister",
        (CanisterIdRecord {
            canister_id: wallet,
        },),
        Wait::Unbounded,
    )
    .await
}

pub async fn start(wallet: Principal) -> Result<(), String> {
    call(
        management(),
        WHO,
        "start_canister",
        (CanisterIdRecord {
            canister_id: wallet,
        },),
        Wait::Unbounded,
    )
    .await
}

/// Removes the wallet's code and state. The IC rejects every call the
/// wallet's code took and has not answered, drops the callbacks of every
/// call it made, and empties its chunk store; its cycles, controllers and
/// history stay.
pub async fn uninstall(wallet: Principal) -> Result<(), String> {
    call(
        management(),
        WHO,
        "uninstall_code",
        (UninstallCodeArgs {
            canister_id: wallet,
            sender_canister_version: Some(ic_cdk::api::canister_version()),
        },),
        Wait::Unbounded,
    )
    .await
}

#[derive(CandidType)]
struct UninstallCodeArgs {
    canister_id: Principal,
    sender_canister_version: Option<u64>,
}

/// Gives the wallet `cycles` of the controller's own.
pub async fn deposit_cycles(wallet: Principal, cycles: u128) -> Result<(), String> {
    call_with_cycles(
        management(),
        WHO,
        "deposit_cycles",
        (CanisterIdRecord {
            canister_id: wallet,
        },),
        Wait::Unbounded,
        cycles,
    )
    .await
}

/// Empties the wallet's chunk store.
pub async fn clear_chunk_store(wallet: Principal) -> Result<(), String> {
    call(
        management(),
        WHO,
        "clear_chunk_store",
        (CanisterIdRecord {
            canister_id: wallet,
        },),
        Wait::Unbounded,
    )
    .await
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

/// Adds a chunk to the wallet's chunk store and returns the hash it is
/// stored under.
pub async fn upload_chunk(wallet: Principal, chunk: Vec<u8>) -> Result<Vec<u8>, String> {
    let hash: ChunkHash = call(
        management(),
        WHO,
        "upload_chunk",
        (UploadChunkArgs {
            canister_id: wallet,
            chunk: ByteBuf::from(chunk),
        },),
        Wait::Unbounded,
    )
    .await?;
    Ok(hash.hash.into_vec())
}

#[derive(CandidType, Deserialize)]
struct UpgradeFlags {
    skip_pre_upgrade: Option<bool>,
}

#[derive(CandidType, Deserialize)]
enum Mode {
    #[serde(rename = "install")]
    Install,
    #[serde(rename = "upgrade")]
    Upgrade(Option<UpgradeFlags>),
}

#[derive(CandidType)]
struct ChunkHashArg {
    hash: ByteBuf,
}

#[derive(CandidType)]
struct InstallChunkedCodeArgs {
    mode: Mode,
    target_canister: Principal,
    store_canister: Option<Principal>,
    chunk_hashes_list: Vec<ChunkHashArg>,
    wasm_module_hash: ByteBuf,
    arg: ByteBuf,
    sender_canister_version: Option<u64>,
}

/// Installs in the wallet the module whose chunks the wallet's own chunk
/// store holds: an upgrade, or for a reinstall an install in the wallet the
/// controller uninstalled. The IC checks the assembled module against
/// `wasm_module_hash` and rejects the whole install, changing nothing, if it
/// differs or the new code's `post_upgrade` or `init` traps.
pub async fn install(
    wallet: Principal,
    mode: InstallMode,
    chunk_hashes: Vec<ByteBuf>,
    wasm_module_hash: ByteBuf,
    arg: ByteBuf,
) -> Result<(), String> {
    let mode = match mode {
        InstallMode::Upgrade { skip_pre_upgrade } => Mode::Upgrade(Some(UpgradeFlags {
            skip_pre_upgrade: Some(skip_pre_upgrade),
        })),
        InstallMode::Reinstall => Mode::Install,
    };
    call(
        management(),
        WHO,
        "install_chunked_code",
        (InstallChunkedCodeArgs {
            mode,
            target_canister: wallet,
            store_canister: Some(wallet),
            chunk_hashes_list: chunk_hashes
                .into_iter()
                .map(|hash| ChunkHashArg { hash })
                .collect(),
            wasm_module_hash,
            arg,
            sender_canister_version: Some(ic_cdk::api::canister_version()),
        },),
        Wait::Unbounded,
    )
    .await
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

/// Makes `controller` the wallet's only controller, which drops this one.
pub async fn hand_over(wallet: Principal, controller: Principal) -> Result<(), String> {
    call(
        management(),
        WHO,
        "update_settings",
        (UpdateSettingsArgs {
            canister_id: wallet,
            settings: Settings {
                controllers: Some(vec![controller]),
            },
            sender_canister_version: Some(ic_cdk::api::canister_version()),
        },),
        Wait::Unbounded,
    )
    .await
}

/// The fields of another controller's `state` this one reads.
#[derive(CandidType, Deserialize)]
pub struct OtherState {
    pub wallet: Principal,
    pub policy_sha256: ByteBuf,
    pub policy_version: u64,
    pub next_nonce: u64,
    pub replaced_by: Option<Principal>,
}

/// Asks another controller what it serves, waiting a bounded time: it is
/// just another canister.
pub async fn other_state(controller: Principal) -> Result<OtherState, String> {
    call(
        controller,
        format!("controller {controller}"),
        "state",
        (),
        Wait::Bounded,
    )
    .await
}

/// Asks another controller about its settings and subnet, waiting a bounded
/// time.
pub async fn other_health(controller: Principal) -> Result<Health, String> {
    call(
        controller,
        format!("controller {controller}"),
        "health",
        (),
        Wait::Bounded,
    )
    .await
}
