//! The factory's interface types (`candid/factory.did`), shared by the
//! factory and the clients that call it.

use crate::Signers;
use candid::{CandidType, Nat, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// The release new wallets get, and the keys that must have signed it. Every
/// wallet the factory creates pins the same channel and keys.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FactoryConfig {
    pub release_channel: Principal,
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum FactoryArg {
    Init(FactoryConfig),
    /// An upgrade keeps the configuration, or replaces it with the one it
    /// names.
    Upgrade(Option<FactoryConfig>),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CreateWalletArgs {
    /// Ed25519 public key of the claim keypair made in the creator's browser.
    pub claim_public_key: ByteBuf,
    pub claim_expires_at_ns: u64,
    /// None installs the channel's latest release.
    pub version: Option<String>,
    /// Cycles the factory pays out of the caller's cycles-ledger account,
    /// which approved the factory for them and the ledger's fee.
    pub cycles: Nat,
    pub from_subaccount: Option<ByteBuf>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CreatedWallet {
    pub wallet: Principal,
    pub version: String,
    /// What the creator paid. The IC's fees to create and install the
    /// wallet came out of it; the wallet's `status` shows the rest.
    pub cycles: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum FactoryError {
    /// An argument out of its form or limit, or an anonymous caller; the text
    /// says which.
    InvalidArgument(String),
    InsufficientAllowance {
        allowance: Nat,
    },
    InsufficientFunds {
        balance: Nat,
    },
    ReleaseUnavailable(String),
    ReleaseRejected(String),
    /// The cycles ledger did not create the canister. It refunds what it
    /// took less its fees, and the text holds its answer.
    CreateFailed(String),
    /// The canister was created and paid for, but the release did not
    /// install. The factory made the creator its only controller.
    InstallFailed {
        wallet: Principal,
        reason: String,
    },
}

/// The fewest cycles a wallet is created with: the IC takes 0.5T to create a
/// canister on a 13-node subnet, and installing the wallet about 0.01T.
pub const MIN_CYCLES: u128 = 1_000_000_000_000;

/// The least time a claim key must have left: the factory takes a few
/// rounds to create and install the wallet, whose install refuses a key
/// that expired.
pub const MIN_CLAIM_WINDOW_NS: u64 = 60 * 60 * 1_000_000_000;

impl FactoryConfig {
    /// Checks the configuration before anything is stored.
    pub fn validate(&self) -> Result<Signers, String> {
        if !is_canister_id(&self.release_channel) {
            return Err("release_channel must be a canister id".to_string());
        }
        Signers::new(&self.release_signers, self.release_threshold)
    }
}

impl CreateWalletArgs {
    /// Checks the arguments before the factory reads the channel or takes a
    /// cycle, and returns the cycles to pay. `now_ns` is the IC time.
    pub fn validate(&self, creator: Principal, now_ns: u64) -> Result<u128, String> {
        if creator == Principal::anonymous() {
            return Err("the anonymous principal cannot create a wallet".to_string());
        }
        if crate::public_key(&self.claim_public_key).is_none() {
            return Err("claim_public_key must be an Ed25519 public key".to_string());
        }
        if self.claim_expires_at_ns < now_ns.saturating_add(MIN_CLAIM_WINDOW_NS) {
            return Err("claim_expires_at_ns must be at least an hour from now".to_string());
        }
        if let Some(version) = &self.version {
            crate::check_version(version)?;
        }
        if self
            .from_subaccount
            .as_ref()
            .is_some_and(|subaccount| subaccount.len() != 32)
        {
            return Err("from_subaccount must be 32 bytes".to_string());
        }
        let cycles = u128::try_from(&self.cycles.0)
            .ok()
            .filter(|&cycles| cycles >= MIN_CYCLES)
            .ok_or_else(|| format!("cycles must be at least {MIN_CYCLES} and fit in 128 bits"))?;
        Ok(cycles)
    }
}

/// Canister ids are opaque principals, whose last byte is 0x01.
pub fn is_canister_id(id: &Principal) -> bool {
    id.as_slice().last() == Some(&0x01)
}
