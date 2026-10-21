//! Release channel: keeps signed wallet releases and serves them in chunks.
//!
//! Publishers stage a release's chunks, then publish its manifest. The
//! manifest goes live, with its chunks, only once its signatures meet the
//! channel's threshold and every chunk matches its hash. Wallets check the
//! same again against the signer keys they pin, so the channel can withhold
//! a release but cannot push a bad one.

mod storage;

use candid::Principal;
use release::channel::{ChannelArg, ChannelError, ChunkRequest, PublishChunk, ReleaseSummary};
use release::{CHUNK_SIZE, MAX_CHUNKS, ReleaseManifest, Signers};
use serde_bytes::ByteBuf;

pub const NOT_A_PUBLISHER: &str = "only a publisher of this channel can do this";

#[ic_cdk::init]
fn init(arg: ChannelArg) {
    let ChannelArg::Init(config) = arg else {
        ic_cdk::trap("install needs ChannelArg::Init");
    };
    if let Err(reason) = config.validate() {
        ic_cdk::trap(format!("invalid ChannelConfig: {reason}"));
    }
    storage::set_config(config);
}

// A trap here makes the IC keep the running version.
#[ic_cdk::post_upgrade]
fn post_upgrade(arg: ChannelArg) {
    let ChannelArg::Upgrade(config) = arg else {
        ic_cdk::trap("an upgrade needs ChannelArg::Upgrade");
    };
    if let Some(config) = config {
        if let Err(reason) = config.validate() {
            ic_cdk::trap(format!("invalid ChannelConfig: {reason}"));
        }
        storage::set_config(config);
    }
    if let Err(reason) = storage::check() {
        ic_cdk::trap(format!("the upgrade cannot read the state: {reason}"));
    }
}

/// Guard: the caller is a publisher. ic-cdk runs it before it decodes the
/// arguments, so a caller the channel refuses never gets it to decode a
/// chunk.
fn publisher() -> Result<(), String> {
    if is_publisher(ic_cdk::api::msg_caller()) {
        Ok(())
    } else {
        Err(NOT_A_PUBLISHER.to_string())
    }
}

fn is_publisher(caller: Principal) -> bool {
    storage::config().publishers.contains(&caller)
}

/// Drops a publish message from anyone else before it reaches the canister,
/// so its bytes cost the channel no cycles. The guard still refuses calls
/// from other canisters, which skip this.
#[ic_cdk::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::msg_method_name();
    if !method.starts_with("publish_") || is_publisher(ic_cdk::api::msg_caller()) {
        ic_cdk::api::accept_message();
    }
}

/// The release published last.
#[ic_cdk::query]
fn latest() -> Option<ReleaseManifest> {
    storage::latest()
}

#[ic_cdk::query]
fn manifest(version: String) -> Option<ReleaseManifest> {
    storage::release(&version)
}

/// Every published release, in the order they were published.
#[ic_cdk::query]
fn releases() -> Vec<ReleaseSummary> {
    storage::releases()
        .into_iter()
        .map(|manifest| ReleaseSummary {
            version: manifest.version,
            wasm_sha256: manifest.wasm_sha256,
            published_at_ns: manifest.published_at_ns,
            notes_url: manifest.notes_url,
        })
        .collect()
}

/// A chunk of a published release; staged chunks are not served.
#[ic_cdk::query]
fn chunk(request: ChunkRequest) -> Option<ByteBuf> {
    if !storage::is_published(&request.version) {
        return None;
    }
    storage::chunk(&request.version, request.index).map(ByteBuf::from)
}

/// Stages a chunk, in place of the one staged at its index before.
#[ic_cdk::update(guard = "publisher")]
fn publish_chunk(args: PublishChunk) -> Result<(), ChannelError> {
    release::check_version(&args.version).map_err(ChannelError::Invalid)?;
    if args.index as usize >= MAX_CHUNKS {
        return Err(ChannelError::Invalid(format!(
            "a release has at most {MAX_CHUNKS} chunks"
        )));
    }
    if args.data.is_empty() || args.data.len() > CHUNK_SIZE {
        return Err(ChannelError::Invalid(format!(
            "a chunk has 1 to {CHUNK_SIZE} bytes"
        )));
    }
    if storage::is_published(&args.version) {
        return Err(ChannelError::AlreadyPublished(args.version));
    }
    storage::stage_chunk(&args.version, args.index, args.data.into_vec());
    Ok(())
}

/// Publishes a release whose chunks are staged. It checks, in this order, the
/// manifest's form, that its version is new, its signatures, and its chunks.
#[ic_cdk::update(guard = "publisher")]
fn publish_manifest(manifest: ReleaseManifest) -> Result<(), ChannelError> {
    manifest.check_form().map_err(ChannelError::Invalid)?;
    if storage::is_published(&manifest.version) {
        return Err(ChannelError::AlreadyPublished(manifest.version));
    }
    let config = storage::config();
    let signers = Signers::new(&config.release_signers, config.release_threshold)
        .expect("the stored configuration was validated");
    signers.check(&manifest)?;
    manifest.check_chunks(|index| storage::chunk(&manifest.version, index as u32))?;
    storage::publish(manifest);
    Ok(())
}

ic_cdk::export_candid!();

#[cfg(test)]
mod tests;
