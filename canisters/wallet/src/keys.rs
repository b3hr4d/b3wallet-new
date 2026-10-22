//! Threshold ECDSA keys of the wallet's accounts. The subnet holds them,
//! never the canister: the management canister derives the public key of a
//! derivation path and signs with its private key.
//!
//! The path of account `n` of a kind, held by the wallet, is
//! `wallet / kind / n`: the bytes `wallet`, the kind's name, and `n` as 4
//! big-endian bytes. A Bitcoin account's kind names its network, so each
//! network has keys of its own; an EVM account's key is its address on every
//! EVM chain. The key itself is the subnet's, by the wallet's Bitcoin
//! network: `key_1` for mainnet, `test_key_1` for testnet, and
//! `dfx_test_key` on a local network and in PocketIC, where regtest runs.

use crate::BitcoinNetwork;
use crate::calls::{self, call};
use candid::{CandidType, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// Curve 0 of `ic0.cost_sign_with_ecdsa`.
const SECP256K1: u32 = 0;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum EcdsaCurve {
    #[serde(rename = "secp256k1")]
    Secp256k1,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct EcdsaKeyId {
    curve: EcdsaCurve,
    name: String,
}

#[derive(CandidType)]
struct EcdsaPublicKeyArgs {
    canister_id: Option<Principal>,
    derivation_path: Vec<ByteBuf>,
    key_id: EcdsaKeyId,
}

#[derive(CandidType, Deserialize)]
struct EcdsaPublicKeyResult {
    public_key: ByteBuf,
    chain_code: ByteBuf,
}

#[derive(CandidType)]
struct SignWithEcdsaArgs {
    message_hash: ByteBuf,
    derivation_path: Vec<ByteBuf>,
    key_id: EcdsaKeyId,
}

#[derive(CandidType, Deserialize)]
struct SignWithEcdsaResult {
    signature: ByteBuf,
}

/// A threshold key and a derivation path under it: one account's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPath {
    key: &'static str,
    path: Vec<ByteBuf>,
}

/// The subnet key of a wallet on `network`.
pub fn key_name(network: BitcoinNetwork) -> &'static str {
    match network {
        BitcoinNetwork::Mainnet => "key_1",
        BitcoinNetwork::Testnet => "test_key_1",
        BitcoinNetwork::Regtest => "dfx_test_key",
    }
}

fn bitcoin_kind(network: BitcoinNetwork) -> &'static [u8] {
    match network {
        BitcoinNetwork::Mainnet => b"bitcoin-mainnet",
        BitcoinNetwork::Testnet => b"bitcoin-testnet",
        BitcoinNetwork::Regtest => b"bitcoin-regtest",
    }
}

fn path(kind: &[u8], index: u32) -> Vec<ByteBuf> {
    vec![
        ByteBuf::from(b"wallet".to_vec()),
        ByteBuf::from(kind.to_vec()),
        ByteBuf::from(index.to_be_bytes().to_vec()),
    ]
}

/// The key of Bitcoin account `index` on `network`, under that network's
/// subnet key.
pub fn bitcoin(network: BitcoinNetwork, index: u32) -> KeyPath {
    KeyPath {
        key: key_name(network),
        path: path(bitcoin_kind(network), index),
    }
}

/// The key of EVM account `index`, under the subnet key of a wallet on
/// `network`.
pub fn evm(network: BitcoinNetwork, index: u32) -> KeyPath {
    KeyPath {
        key: key_name(network),
        path: path(b"evm", index),
    }
}

impl KeyPath {
    fn key_id(&self) -> EcdsaKeyId {
        EcdsaKeyId {
            curve: EcdsaCurve::Secp256k1,
            name: self.key.to_string(),
        }
    }
}

const MANAGEMENT: &str = "the management canister";

/// The compressed SEC1 public key of `key`.
pub async fn public_key(key: &KeyPath) -> Result<Vec<u8>, String> {
    let arg = EcdsaPublicKeyArgs {
        canister_id: None,
        derivation_path: key.path.clone(),
        key_id: key.key_id(),
    };
    let result: EcdsaPublicKeyResult = call(
        Principal::management_canister(),
        MANAGEMENT,
        "ecdsa_public_key",
        arg,
        0,
    )
    .await
    .map_err(calls::reason)?;
    Ok(result.public_key.into_vec())
}

/// The signature of `hash` by `key`, as 64 bytes of `r` and `s`. Signing
/// changes nothing, so a failure is only an error.
pub async fn sign(key: &KeyPath, hash: [u8; 32]) -> Result<Vec<u8>, String> {
    let cycles = ic_cdk::api::cost_sign_with_ecdsa(key.key, SECP256K1)
        .map_err(|error| format!("no signing cost for the key {}: {error}", key.key))?;
    let arg = SignWithEcdsaArgs {
        message_hash: ByteBuf::from(hash.to_vec()),
        derivation_path: key.path.clone(),
        key_id: key.key_id(),
    };
    let result: SignWithEcdsaResult = call(
        Principal::management_canister(),
        MANAGEMENT,
        "sign_with_ecdsa",
        arg,
        cycles,
    )
    .await
    .map_err(calls::reason)?;
    Ok(result.signature.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(key: &KeyPath) -> Vec<&[u8]> {
        key.path.iter().map(|part| part.as_slice()).collect()
    }

    #[test]
    fn a_path_is_wallet_then_the_kind_then_the_index() {
        let key = bitcoin(BitcoinNetwork::Regtest, 0x0102_0304);
        assert_eq!(
            parts(&key),
            [&b"wallet"[..], b"bitcoin-regtest", &[1, 2, 3, 4]]
        );
        assert_ne!(
            bitcoin(BitcoinNetwork::Mainnet, 0).path,
            bitcoin(BitcoinNetwork::Testnet, 0).path
        );
        assert_ne!(
            bitcoin(BitcoinNetwork::Mainnet, 0),
            bitcoin(BitcoinNetwork::Mainnet, 1)
        );
        assert_eq!(
            parts(&evm(BitcoinNetwork::Regtest, 7)),
            [&b"wallet"[..], b"evm", &[0, 0, 0, 7]]
        );
    }

    #[test]
    fn each_network_signs_with_its_subnet_key() {
        assert_eq!(bitcoin(BitcoinNetwork::Mainnet, 0).key, "key_1");
        assert_eq!(bitcoin(BitcoinNetwork::Testnet, 0).key, "test_key_1");
        assert_eq!(bitcoin(BitcoinNetwork::Regtest, 0).key, "dfx_test_key");
        assert_eq!(evm(BitcoinNetwork::Mainnet, 0).key, "key_1");
        assert_eq!(evm(BitcoinNetwork::Testnet, 0).key, "test_key_1");
    }
}
