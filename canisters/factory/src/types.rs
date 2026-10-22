//! The wallet's install arguments as the factory sends them, and the
//! factory's interface types from the `release` crate.

use candid::{CandidType, Principal};
pub use release::factory::*;
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// The wallet's `InitArgs` (`candid/wallet.did`).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WalletInitArgs {
    pub claim_public_key: ByteBuf,
    pub claim_expires_at_ns: u64,
    pub release_channel: Principal,
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
    pub factory: Option<Principal>,
    /// None: the factory makes mainnet wallets.
    pub bitcoin_network: Option<BitcoinNetwork>,
}

/// The wallet's `BitcoinNetwork`.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Regtest,
}

/// The wallet's `UpgradeArgs`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WalletUpgradeArgs {
    pub bitcoin_network: Option<BitcoinNetwork>,
}

/// The wallet's `WalletArg`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum WalletArg {
    Init(WalletInitArgs),
    Upgrade(Option<WalletUpgradeArgs>),
}
