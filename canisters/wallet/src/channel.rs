//! Calls to the release channel the wallet was installed with. Its answers
//! are checked like any stranger's: a manifest by the signatures the wallet
//! pins, a chunk by the hash in that manifest.

use crate::calls::{self, call, call_without_args};
use candid::Principal;
use release::ReleaseManifest;
use release::channel::ChunkRequest;
use serde_bytes::ByteBuf;

fn who(channel: Principal) -> String {
    format!("the release channel {channel}")
}

/// The release the channel published last.
pub async fn latest(channel: Principal) -> Result<Option<ReleaseManifest>, String> {
    call_without_args(channel, who(channel), "latest")
        .await
        .map_err(calls::reason)
}

/// The published release `version`.
pub async fn manifest(
    channel: Principal,
    version: &str,
) -> Result<Option<ReleaseManifest>, String> {
    call(channel, who(channel), "manifest", version, 0)
        .await
        .map_err(calls::reason)
}

/// Chunk `index` of the published release `version`.
pub async fn chunk(
    channel: Principal,
    version: &str,
    index: u32,
) -> Result<Option<Vec<u8>>, String> {
    let request = ChunkRequest {
        version: version.to_string(),
        index,
    };
    let chunk: Option<ByteBuf> = call(channel, who(channel), "chunk", request, 0)
        .await
        .map_err(calls::reason)?;
    Ok(chunk.map(ByteBuf::into_vec))
}
