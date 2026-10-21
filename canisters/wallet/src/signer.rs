//! Threshold ECDSA keys of the wallet's Bitcoin accounts. The subnet holds
//! them, never the canister: the management canister derives the public key
//! of a derivation path and signs with its private key.
//!
//! The path of account index `n` on a chain, held by the wallet, is
//! `wallet / chain / n`: the bytes `wallet`, the chain's name, and `n` as 4
//! big-endian bytes.

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

/// The threshold key of a Bitcoin network: the IC's production key for
/// mainnet, its test key for testnet, and the key of a local network and of
/// PocketIC for regtest.
pub fn key_name(network: BitcoinNetwork) -> &'static str {
    match network {
        BitcoinNetwork::Mainnet => "key_1",
        BitcoinNetwork::Testnet => "test_key_1",
        BitcoinNetwork::Regtest => "dfx_test_key",
    }
}

fn chain_name(network: BitcoinNetwork) -> &'static [u8] {
    match network {
        BitcoinNetwork::Mainnet => b"bitcoin-mainnet",
        BitcoinNetwork::Testnet => b"bitcoin-testnet",
        BitcoinNetwork::Regtest => b"bitcoin-regtest",
    }
}

/// The derivation path of account `index` on `network`.
pub fn derivation_path(network: BitcoinNetwork, index: u32) -> Vec<ByteBuf> {
    vec![
        ByteBuf::from(b"wallet".to_vec()),
        ByteBuf::from(chain_name(network).to_vec()),
        ByteBuf::from(index.to_be_bytes().to_vec()),
    ]
}

fn key_id(network: BitcoinNetwork) -> EcdsaKeyId {
    EcdsaKeyId {
        curve: EcdsaCurve::Secp256k1,
        name: key_name(network).to_string(),
    }
}

const MANAGEMENT: &str = "the management canister";

/// The compressed SEC1 public key of account `index` on `network`.
pub async fn public_key(network: BitcoinNetwork, index: u32) -> Result<Vec<u8>, String> {
    let arg = EcdsaPublicKeyArgs {
        canister_id: None,
        derivation_path: derivation_path(network, index),
        key_id: key_id(network),
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

/// The signature of `hash` by the key of account `index` on `network`, as
/// 64 bytes of `r` and `s`. Signing changes nothing, so a failure is only
/// an error.
pub async fn sign(network: BitcoinNetwork, index: u32, hash: [u8; 32]) -> Result<Vec<u8>, String> {
    let key = key_name(network);
    let cycles = ic_cdk::api::cost_sign_with_ecdsa(key, SECP256K1)
        .map_err(|error| format!("no signing cost for the key {key}: {error}"))?;
    let arg = SignWithEcdsaArgs {
        message_hash: ByteBuf::from(hash.to_vec()),
        derivation_path: derivation_path(network, index),
        key_id: key_id(network),
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

    #[test]
    fn a_path_is_wallet_then_the_chain_then_the_index() {
        let path = derivation_path(BitcoinNetwork::Regtest, 0x0102_0304);
        assert_eq!(
            path.iter().map(|part| part.as_slice()).collect::<Vec<_>>(),
            [&b"wallet"[..], b"bitcoin-regtest", &[1, 2, 3, 4]]
        );
        assert_ne!(
            derivation_path(BitcoinNetwork::Mainnet, 0),
            derivation_path(BitcoinNetwork::Testnet, 0)
        );
        assert_ne!(
            derivation_path(BitcoinNetwork::Mainnet, 0),
            derivation_path(BitcoinNetwork::Mainnet, 1)
        );
    }

    #[test]
    fn each_network_signs_with_its_key() {
        assert_eq!(key_name(BitcoinNetwork::Mainnet), "key_1");
        assert_eq!(key_name(BitcoinNetwork::Testnet), "test_key_1");
        assert_eq!(key_name(BitcoinNetwork::Regtest), "dfx_test_key");
    }
}
