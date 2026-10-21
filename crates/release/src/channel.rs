//! The release channel's interface types (`candid/release_channel.did`),
//! shared by the channel, the wallets that read it, and the release tool.

use crate::{ChunkError, NotEnoughSignatures, Signers};
use candid::{CandidType, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// The most publishers a channel has.
pub const MAX_PUBLISHERS: usize = 16;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChannelConfig {
    pub publishers: Vec<Principal>,
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ChannelArg {
    Init(ChannelConfig),
    Upgrade(Option<ChannelConfig>),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSummary {
    pub version: String,
    pub wasm_sha256: ByteBuf,
    pub published_at_ns: u64,
    pub notes_url: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChunkRequest {
    pub version: String,
    pub index: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PublishChunk {
    pub version: String,
    pub index: u32,
    pub data: ByteBuf,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ChannelError {
    Invalid(String),
    MissingChunk { index: u32 },
    HashMismatch { index: u32 },
    WasmMismatch,
    NotEnoughSignatures { have: u8, need: u8 },
    AlreadyPublished(String),
}

impl From<ChunkError> for ChannelError {
    fn from(error: ChunkError) -> Self {
        match error {
            ChunkError::Missing { index } => ChannelError::MissingChunk { index },
            ChunkError::HashMismatch { index } => ChannelError::HashMismatch { index },
            ChunkError::WasmMismatch => ChannelError::WasmMismatch,
        }
    }
}

impl From<NotEnoughSignatures> for ChannelError {
    fn from(error: NotEnoughSignatures) -> Self {
        ChannelError::NotEnoughSignatures {
            have: error.have,
            need: error.need,
        }
    }
}

impl ChannelConfig {
    /// Checks the configuration before anything is stored. Nobody publishing
    /// is allowed: such a channel keeps serving what it has.
    pub fn validate(&self) -> Result<Signers, String> {
        if self.publishers.len() > MAX_PUBLISHERS {
            return Err(format!("at most {MAX_PUBLISHERS} publishers"));
        }
        if self.publishers.contains(&Principal::anonymous()) {
            return Err("the anonymous principal cannot publish".to_string());
        }
        Signers::new(&self.release_signers, self.release_threshold)
    }
}
