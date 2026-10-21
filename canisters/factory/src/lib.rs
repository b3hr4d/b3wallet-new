//! Factory: creates a wallet canister, installs a signed release in it, and
//! makes the wallet its own only controller. It keeps no rights over any
//! wallet it created.
//!
//! The creator pays: the cycles ledger takes the wallet's cycles out of the
//! creator's account, under an allowance the creator gave the factory. The
//! factory checks the release's signatures before anyone pays, and each chunk
//! against the signed manifest before it uploads it.
//!
//! Until it hands a canister over, the factory keeps a record of it in stable
//! memory. A hand-over that fails is tried again by the IC's global timer, so
//! the factory gives up every canister it created even if one call fails or
//! a callback traps: a canister that runs the release goes to itself, one
//! that does not goes to its creator.

mod calls;
mod channel;
mod cycles_ledger;
mod management;
mod storage;
mod types;

pub use types::*;

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use release::{ChunkError, ReleaseManifest};
use std::cell::RefCell;
use std::collections::BTreeSet;
use storage::HandOver;

#[ic_cdk::init]
fn init(arg: FactoryArg) {
    let FactoryArg::Init(config) = arg else {
        ic_cdk::trap("install needs FactoryArg::Init");
    };
    if let Err(reason) = config.validate() {
        ic_cdk::trap(format!("invalid FactoryConfig: {reason}"));
    }
    storage::set_config(config);
}

// A trap here makes the IC keep the running version.
#[ic_cdk::post_upgrade]
fn post_upgrade(arg: FactoryArg) {
    let FactoryArg::Upgrade(config) = arg else {
        ic_cdk::trap("an upgrade needs FactoryArg::Upgrade");
    };
    if let Some(config) = config {
        if let Err(reason) = config.validate() {
            ic_cdk::trap(format!("invalid FactoryConfig: {reason}"));
        }
        storage::set_config(config);
    }
    if let Err(reason) = storage::check() {
        ic_cdk::trap(format!("the upgrade cannot read the state: {reason}"));
    }
    // The factory was stopped to be upgraded, so no creation is under way,
    // and every canister still on record waits for the timer.
    schedule_timer();
}

/// The release a wallet created now gets: the channel's latest, once the
/// factory checked its form and its signatures. An update, since it reads
/// the channel.
#[ic_cdk::update]
async fn current_release() -> Result<ReleaseManifest, FactoryError> {
    checked_release(&storage::config(), None).await
}

/// Creates a wallet for the claim key in `args`, paid by the caller, and
/// hands it to itself.
#[ic_cdk::update]
async fn create_wallet(args: CreateWalletArgs) -> Result<CreatedWallet, FactoryError> {
    let creator = ic_cdk::api::msg_caller();
    let cycles = args
        .validate(creator, ic_cdk::api::time())
        .map_err(FactoryError::InvalidArgument)?;
    let config = storage::config();
    let manifest = checked_release(&config, args.version.as_deref()).await?;
    let from = Account {
        owner: creator,
        subaccount: args.from_subaccount.as_ref().map(|subaccount| {
            subaccount
                .as_slice()
                .try_into()
                .expect("a validated subaccount is 32 bytes")
        }),
    };
    let wallet = cycles_ledger::create_canister_from(from, cycles).await?;
    // Recorded in the message that learns of the canister, so no trap after
    // it leaves a canister the factory controls and does not know of; the
    // timer hands it over if this creation never does.
    storage::owe_hand_over(
        wallet,
        HandOver {
            creator,
            wasm_sha256: manifest.wasm_sha256.clone(),
            created_at_ns: ic_cdk::api::time(),
        },
    );
    schedule_timer();
    let busy = Busy::start(wallet);
    let installed = install(wallet, &config, &manifest, &args).await;
    let owner = if installed.is_ok() { wallet } else { creator };
    // A failed hand-over stays on record for the timer.
    let _ = hand_over(wallet, owner).await;
    drop(busy);
    schedule_timer();
    match installed {
        Ok(()) => Ok(CreatedWallet {
            wallet,
            version: manifest.version,
            cycles: args.cycles,
        }),
        Err(reason) => Err(FactoryError::InstallFailed { wallet, reason }),
    }
}

/// The release `version`, or the latest, once its form and its signatures
/// pass.
async fn checked_release(
    config: &FactoryConfig,
    version: Option<&str>,
) -> Result<ReleaseManifest, FactoryError> {
    let channel = config.release_channel;
    let found = match version {
        Some(version) => channel::manifest(channel, version).await,
        None => channel::latest(channel).await,
    }
    .map_err(FactoryError::ReleaseUnavailable)?;
    let manifest = found.ok_or_else(|| {
        FactoryError::ReleaseUnavailable(match version {
            Some(version) => format!("the release channel has no release {version}"),
            None => "the release channel has published no release".to_string(),
        })
    })?;
    if let Some(version) = version
        && manifest.version != version
    {
        return Err(FactoryError::ReleaseRejected(format!(
            "the release channel answered release {} for {version}",
            manifest.version
        )));
    }
    manifest.check_form().map_err(|reason| {
        FactoryError::ReleaseRejected(format!(
            "the manifest of {} is out of form: {reason}",
            manifest.version
        ))
    })?;
    let signers = config
        .validate()
        .expect("the stored configuration was validated");
    signers.check(&manifest).map_err(|missing| {
        FactoryError::ReleaseRejected(format!(
            "release {} has {} of the {} signatures it needs",
            manifest.version, missing.have, missing.need
        ))
    })?;
    Ok(manifest)
}

/// Stages the release's chunks in the new canister's own chunk store, each
/// checked against the signed manifest, and installs the wallet from them.
async fn install(
    wallet: Principal,
    config: &FactoryConfig,
    manifest: &ReleaseManifest,
    args: &CreateWalletArgs,
) -> Result<(), String> {
    let version = &manifest.version;
    for (index, hash) in manifest.chunk_hashes.iter().enumerate() {
        let data = channel::chunk(config.release_channel, version, index as u32)
            .await?
            .ok_or_else(|| format!("the release channel has no chunk {index} of {version}"))?;
        manifest
            .check_chunk(index, &data)
            .map_err(|error| chunk_refused(version, index, error))?;
        let stored = management::upload_chunk(wallet, data).await?;
        if stored != hash.as_slice() {
            return Err(format!(
                "the chunk store keeps chunk {index} of {version} under another hash"
            ));
        }
    }
    let arg = WalletArg::Init(WalletInitArgs {
        claim_public_key: args.claim_public_key.clone(),
        claim_expires_at_ns: args.claim_expires_at_ns,
        release_channel: config.release_channel,
        release_signers: config.release_signers.clone(),
        release_threshold: config.release_threshold,
        factory: Some(ic_cdk::api::canister_self()),
    });
    // The IC checks that the chunks make up the module the signers named.
    management::install_chunked_code(
        wallet,
        manifest.chunk_hashes.clone(),
        manifest.wasm_sha256.clone(),
        candid::encode_one(arg).expect("WalletArg encodes"),
    )
    .await
}

fn chunk_refused(version: &str, index: usize, error: ChunkError) -> String {
    match error {
        ChunkError::HashMismatch { .. } => {
            format!("chunk {index} of {version} does not match its hash in the signed manifest")
        }
        ChunkError::WasmMismatch | ChunkError::Missing { .. } => {
            format!("chunk {index} of {version} is not cut where the signed manifest cuts the Wasm")
        }
    }
}

/// Empties the canister's chunk store, which the release needed only to
/// install, and makes `owner` its only controller. The record of the
/// canister goes once the factory no longer controls it.
async fn hand_over(canister: Principal, owner: Principal) -> Result<(), String> {
    management::clear_chunk_store(canister).await?;
    management::hand_over(canister, owner).await?;
    storage::handed_over(canister);
    Ok(())
}

thread_local! {
    /// The canisters a creation or the timer works on now, which the timer
    /// leaves alone. On the heap: an upgrade needs the factory stopped, so
    /// none is under way when a new version starts.
    static BUSY: RefCell<BTreeSet<Principal>> = const { RefCell::new(BTreeSet::new()) };
}

/// Marks a canister busy until it is dropped, also when a callback traps:
/// ic-cdk then drops the call's future, and with it this guard, and keeps
/// what the guard writes.
struct Busy(Principal);

impl Busy {
    /// None if the canister is busy already. The guard is made only once the
    /// canister is marked: a guard dropped here would unmark it for the
    /// creation or timer pass that holds it.
    fn try_start(canister: Principal) -> Option<Busy> {
        BUSY.with_borrow_mut(|busy| busy.insert(canister))
            .then(|| Busy(canister))
    }

    fn start(canister: Principal) -> Busy {
        Busy::try_start(canister).expect("a new canister is not busy")
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.with_borrow_mut(|busy| busy.remove(&self.0));
    }
}

/// How long the timer waits before it tries a failed hand-over again.
const RETRY_NS: u64 = 5 * 60 * 1_000_000_000;

/// Sets the IC's one timer while a canister waits for its hand-over, or
/// turns it off. A timer already set keeps its deadline, so creations that
/// follow each other cannot keep pushing a retry back.
fn schedule_timer() {
    if storage::hand_overs().is_empty() {
        ic_cdk::api::global_timer_set(0);
        return;
    }
    let at = ic_cdk::api::time().saturating_add(RETRY_NS);
    let previous = ic_cdk::api::global_timer_set(at);
    let deadline = next_retry(previous, at);
    if deadline != at {
        ic_cdk::api::global_timer_set(deadline);
    }
}

/// The timer's deadline: `at`, unless the timer was already set (`previous`
/// is not 0) for earlier.
fn next_retry(previous: u64, at: u64) -> u64 {
    if previous == 0 { at } else { previous.min(at) }
}

/// Hands over every canister on record that no creation works on: to itself
/// if it runs the release it was created for, else to its creator. A
/// canister the factory no longer controls only leaves the record.
async fn retry_hand_overs() {
    let me = ic_cdk::api::canister_self();
    for (canister, record) in storage::hand_overs() {
        let Some(_busy) = Busy::try_start(canister) else {
            continue;
        };
        let Ok(info) = management::info(canister).await else {
            continue;
        };
        if !info.controllers.contains(&me) {
            storage::handed_over(canister);
            continue;
        }
        let owner = owner_of(
            canister,
            &record,
            info.module_hash.as_ref().map(|hash| hash.as_slice()),
        );
        let _ = hand_over(canister, owner).await;
    }
    schedule_timer();
}

/// Who gets a canister the factory created for `record`.
fn owner_of(canister: Principal, record: &HandOver, module_hash: Option<&[u8]>) -> Principal {
    if module_hash == Some(record.wasm_sha256.as_slice()) {
        canister
    } else {
        record.creator
    }
}

#[cfg_attr(target_family = "wasm", unsafe(export_name = "canister_global_timer"))]
#[cfg_attr(
    not(target_family = "wasm"),
    unsafe(export_name = "canister_global_timer_host")
)]
fn global_timer() {
    ic_cdk::futures::internals::in_executor_context(|| {
        ic_cdk::futures::spawn(retry_hand_overs());
    });
}

ic_cdk::export_candid!();

#[cfg(test)]
mod tests;
