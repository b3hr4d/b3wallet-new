//! Bitcoin accounts and transactions: an account's address, what `propose`
//! checks in a Bitcoin transfer, which outputs a transfer spends, and the
//! transaction it signs. Pure: `signer` and `bitcoin_canister` make the calls
//! and `actions` records how they ended.
//!
//! An account's coins are held by the P2WPKH address of its threshold ECDSA
//! key. A transfer spends the account's confirmed outputs, largest first,
//! pays the recipient in its first output and the change back to the account
//! in its second. Its id covers neither signature, so signing the same
//! transaction again cannot make a second one.

use crate::{BitcoinNetwork, TransferRequest, WalletError};
use bitcoin::consensus::encode;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Message, Secp256k1, ecdsa};
use bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bitcoin::transaction::Version;
use bitcoin::{
    Address, Amount, CompressedPublicKey, Network, OutPoint, Script, ScriptBuf, Sequence,
    Transaction, TxIn, TxOut, Txid, Witness, absolute::LockTime,
};
use candid::Nat;
use std::str::FromStr;

/// All the satoshis there will ever be.
pub const MAX_SATOSHIS: u64 = 21_000_000 * 100_000_000;
/// At most this many outputs are spent at once: each needs a signature.
pub const MAX_INPUTS: usize = 32;
/// The least fee rate the wallet pays, in millisatoshi per virtual byte:
/// one satoshi per vbyte, ten times the relay minimum of Bitcoin Core 31.
pub const MIN_FEE_RATE: u64 = 1_000;
/// The fee rate when the Bitcoin canister has seen too few transactions to
/// offer one, as on a fresh regtest network.
pub const DEFAULT_FEE_RATE: u64 = 2_000;

/// The weight of a P2WPKH input: outpoint, empty script and sequence, then a
/// witness of a signature in its longest DER form, its sighash byte and a
/// compressed public key.
const INPUT_WEIGHT: u64 = (32 + 4 + 1 + 4) * 4 + (1 + 1 + 73 + 1 + 33);
/// Version, lock time, and the input and output counts, with the segwit
/// marker and flag.
const BASE_WEIGHT: u64 = (4 + 4 + 1 + 1) * 4 + 2;

/// The network an account's chain names.
pub fn network(network: BitcoinNetwork) -> Network {
    match network {
        BitcoinNetwork::Mainnet => Network::Bitcoin,
        // The Bitcoin canister follows testnet4, whose addresses read as
        // testnet's.
        BitcoinNetwork::Testnet => Network::Testnet,
        BitcoinNetwork::Regtest => Network::Regtest,
    }
}

/// The P2WPKH address of a compressed SEC1 public key.
pub fn address(public_key: &[u8], on: BitcoinNetwork) -> Result<Address, String> {
    let key = CompressedPublicKey::from_slice(public_key)
        .map_err(|error| format!("not a compressed public key: {error}"))?;
    Ok(Address::p2wpkh(&key, network(on)))
}

/// A Bitcoin transfer as `propose` and `execute` check it, in satoshis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub to: Address,
    pub amount: u64,
    /// The whole fee, when the proposer fixed it.
    pub fee: Option<u64>,
}

fn satoshis(value: &Nat, what: &str) -> Result<u64, WalletError> {
    u64::try_from(&value.0)
        .ok()
        .filter(|sats| *sats <= MAX_SATOSHIS)
        .ok_or_else(|| {
            WalletError::InvalidArgument(format!("{what} is more satoshis than there are"))
        })
}

/// Checks a transfer from an account on `on`: a recipient address of that
/// network, an amount the network relays, and no memo.
pub fn check(request: &TransferRequest, on: BitcoinNetwork) -> Result<Checked, WalletError> {
    let to = Address::from_str(&request.to)
        .map_err(|error| {
            WalletError::InvalidArgument(format!("`to` is not a Bitcoin address: {error}"))
        })?
        .require_network(network(on))
        .map_err(|_| {
            WalletError::InvalidArgument(format!("`to` is not an address of Bitcoin {on:?}"))
        })?;
    let amount = satoshis(&request.amount, "the amount")?;
    let dust = to.script_pubkey().minimal_non_dust().to_sat();
    if amount < dust {
        return Err(WalletError::InvalidArgument(format!(
            "the network relays no output to that address below {dust} satoshis"
        )));
    }
    let fee = request
        .fee
        .as_ref()
        .map(|fee| satoshis(fee, "the fee"))
        .transpose()?;
    if request.memo.is_some() {
        return Err(WalletError::InvalidArgument(
            "a Bitcoin transfer carries no memo".to_string(),
        ));
    }
    Ok(Checked { to, amount, fee })
}

/// The median of the fee rates the Bitcoin canister reports, in millisatoshi
/// per vbyte, and never below `MIN_FEE_RATE`.
pub fn fee_rate(percentiles: &[u64]) -> u64 {
    percentiles
        .get(percentiles.len() / 2)
        .copied()
        .unwrap_or(DEFAULT_FEE_RATE)
        .max(MIN_FEE_RATE)
}

fn output_weight(script: &Script) -> u64 {
    // Scripts of standard outputs are under 253 bytes: a one-byte length.
    (8 + 1 + script.len() as u64) * 4
}

/// The virtual size of a transaction spending `inputs` P2WPKH outputs into
/// `outputs`, with signatures at their longest, so it is never less than
/// the signed transaction's.
pub fn vsize(inputs: usize, outputs: &[&Script]) -> u64 {
    let weight = BASE_WEIGHT
        + inputs as u64 * INPUT_WEIGHT
        + outputs
            .iter()
            .map(|script| output_weight(script))
            .sum::<u64>();
    weight.div_ceil(4)
}

/// An output the account can spend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coin {
    /// The transaction id in its internal byte order, as the Bitcoin canister
    /// returns it.
    pub txid: [u8; 32],
    pub vout: u32,
    pub value: u64,
}

impl Coin {
    /// The outpoint as 36 bytes: the transaction id, then the output index
    /// in big-endian order, so the bytes sort as the outpoints do.
    pub fn key(&self) -> [u8; 36] {
        let mut key = [0; 36];
        key[..32].copy_from_slice(&self.txid);
        key[32..].copy_from_slice(&self.vout.to_be_bytes());
        key
    }
}

/// How the fee is set: the proposer's whole fee, or a rate in millisatoshi
/// per vbyte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fee {
    Total(u64),
    Rate(u64),
}

/// The outputs a transfer spends and what it pays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub inputs: Vec<Coin>,
    pub fee: u64,
    /// What goes back to the account; 0 for no change output.
    pub change: u64,
}

/// Chooses the outputs that pay `amount` to `to` and the fee, largest first.
/// Change too small for the network to relay goes to the fee as well.
pub fn plan(
    mut coins: Vec<Coin>,
    amount: u64,
    to: &Script,
    change: &Script,
    fee: Fee,
) -> Result<Plan, String> {
    coins.sort_by(|a, b| b.value.cmp(&a.value).then(a.key().cmp(&b.key())));
    let available: u64 = coins.iter().map(|coin| coin.value).sum();
    let dust = change.minimal_non_dust().to_sat();
    let fee_for = |vsize: u64| -> Result<u64, String> {
        match fee {
            Fee::Rate(rate) => Ok((rate * vsize).div_ceil(1_000)),
            Fee::Total(total) if total.saturating_mul(1_000) >= MIN_FEE_RATE * vsize => Ok(total),
            Fee::Total(total) => Err(format!(
                "a fee of {total} satoshis is below {} satoshi per vbyte for this \
                 transaction's {vsize} vbytes",
                MIN_FEE_RATE / 1_000
            )),
        }
    };
    let mut total = 0u64;
    for (count, coin) in coins.iter().enumerate().take(MAX_INPUTS) {
        total += coin.value;
        let inputs = count + 1;
        let with_change = fee_for(vsize(inputs, &[to, change]))?;
        if let Some(left) = total.checked_sub(amount + with_change)
            && left >= dust
        {
            return Ok(Plan {
                inputs: coins[..inputs].to_vec(),
                fee: with_change,
                change: left,
            });
        }
        let without = fee_for(vsize(inputs, &[to]))?;
        if total >= amount + without {
            return Ok(Plan {
                inputs: coins[..inputs].to_vec(),
                fee: total - amount,
                change: 0,
            });
        }
    }
    if coins.len() > MAX_INPUTS {
        return Err(format!(
            "the transfer would spend more than {MAX_INPUTS} outputs; send less, \
             or send the account's coins to itself in parts first"
        ));
    }
    Err(format!(
        "the account can spend {available} satoshis, less than the amount and the fee"
    ))
}

/// The transaction of a plan, without signatures: the recipient's output,
/// then the change.
pub fn unsigned(plan: &Plan, amount: u64, to: &Script, change: &Script) -> Transaction {
    let mut output = vec![TxOut {
        value: Amount::from_sat(amount),
        script_pubkey: to.to_owned(),
    }];
    if plan.change > 0 {
        output.push(TxOut {
            value: Amount::from_sat(plan.change),
            script_pubkey: change.to_owned(),
        });
    }
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: plan
            .inputs
            .iter()
            .map(|coin| TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array(coin.txid),
                    vout: coin.vout,
                },
                script_sig: ScriptBuf::new(),
                // Signals replaceability, so a later release can raise the fee.
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            })
            .collect(),
        output,
    }
}

pub fn to_bytes(tx: &Transaction) -> Vec<u8> {
    encode::serialize(tx)
}

pub fn from_bytes(bytes: &[u8]) -> Result<Transaction, String> {
    encode::deserialize(bytes)
        .map_err(|error| format!("a stored transaction does not decode: {error}"))
}

/// The transaction id as block explorers and Bitcoin Core show it.
pub fn txid(tx: &Transaction) -> String {
    tx.compute_txid().to_string()
}

/// The outpoints a transaction spends, as `Coin::key` writes them.
pub fn spent_keys(tx: &Transaction) -> Vec<[u8; 36]> {
    tx.input
        .iter()
        .map(|input| {
            Coin {
                txid: input.previous_output.txid.to_byte_array(),
                vout: input.previous_output.vout,
                value: 0,
            }
            .key()
        })
        .collect()
}

/// What each input's signature signs: its BIP-143 hash, which covers the
/// value of the output it spends.
pub fn sighashes(tx: &Transaction, values: &[u64], own: &Script) -> Result<Vec<[u8; 32]>, String> {
    if values.len() != tx.input.len() {
        return Err("a value for each input is missing".to_string());
    }
    let mut cache = SighashCache::new(tx);
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            cache
                .p2wpkh_signature_hash(index, own, Amount::from_sat(*value), EcdsaSighashType::All)
                .map(|hash| hash.to_byte_array())
                .map_err(|error| format!("no signature hash for input {index}: {error}"))
        })
        .collect()
}

/// The transaction with a witness for each input, from the 64-byte `r || s`
/// signatures of `sighashes` by `public_key`. Each signature is checked first:
/// the Bitcoin canister accepts a transaction the network will refuse.
pub fn signed(
    mut tx: Transaction,
    sighashes: &[[u8; 32]],
    signatures: &[Vec<u8>],
    public_key: &[u8],
) -> Result<Transaction, String> {
    if sighashes.len() != tx.input.len() || signatures.len() != tx.input.len() {
        return Err("a signature for each input is missing".to_string());
    }
    let key = CompressedPublicKey::from_slice(public_key)
        .map_err(|error| format!("not a compressed public key: {error}"))?;
    let verifier = Secp256k1::verification_only();
    for (index, input) in tx.input.iter_mut().enumerate() {
        let mut signature = ecdsa::Signature::from_compact(&signatures[index])
            .map_err(|error| format!("signature {index} is not 64 bytes of r and s: {error}"))?;
        // The network relays only the low-s form.
        signature.normalize_s();
        verifier
            .verify_ecdsa(&Message::from_digest(sighashes[index]), &signature, &key.0)
            .map_err(|_| format!("signature {index} does not verify under the account's key"))?;
        input.witness = Witness::p2wpkh(
            &bitcoin::ecdsa::Signature {
                signature,
                sighash_type: EcdsaSighashType::All,
            },
            &key.0,
        );
    }
    Ok(tx)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bitcoin::secp256k1::SecretKey;
    use serde_bytes::ByteBuf;

    const REGTEST: BitcoinNetwork = BitcoinNetwork::Regtest;

    fn secret(seed: u8) -> SecretKey {
        SecretKey::from_slice(&[seed; 32]).unwrap()
    }

    fn public(seed: u8) -> Vec<u8> {
        let secp = Secp256k1::signing_only();
        secret(seed).public_key(&secp).serialize().to_vec()
    }

    fn own() -> Address {
        address(&public(1), REGTEST).unwrap()
    }

    fn friend() -> Address {
        address(&public(2), REGTEST).unwrap()
    }

    fn request(to: &str, amount: u64) -> TransferRequest {
        TransferRequest {
            from: 1,
            to: to.to_string(),
            amount: Nat::from(amount),
            fee: None,
            memo: None,
        }
    }

    fn coin(seed: u8, value: u64) -> Coin {
        Coin {
            txid: [seed; 32],
            vout: u32::from(seed),
            value,
        }
    }

    /// BIP-173's example key.
    pub const BIP173_KEY: &str =
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    /// Its regtest address.
    pub const BIP173_REGTEST: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

    #[test]
    fn an_address_is_the_p2wpkh_of_the_key_on_its_network() {
        let key = hex(BIP173_KEY);
        assert_eq!(
            address(&key, BitcoinNetwork::Mainnet).unwrap().to_string(),
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"
        );
        assert_eq!(
            address(&key, BitcoinNetwork::Regtest).unwrap().to_string(),
            BIP173_REGTEST
        );
        assert!(
            address(&key, BitcoinNetwork::Testnet)
                .unwrap()
                .to_string()
                .starts_with("tb1q")
        );
        assert!(address(&key[1..], REGTEST).is_err());
    }

    pub fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn a_transfer_needs_an_address_of_its_network_a_relayed_amount_and_no_memo() {
        let to = friend().to_string();
        assert_eq!(
            check(&request(&to, 1_000), REGTEST),
            Ok(Checked {
                to: friend(),
                amount: 1_000,
                fee: None
            })
        );
        let mainnet = address(&public(2), BitcoinNetwork::Mainnet)
            .unwrap()
            .to_string();
        let mut with_memo = request(&to, 1_000);
        with_memo.memo = Some(ByteBuf::from(vec![1]));
        let mut huge_fee = request(&to, 1_000);
        huge_fee.fee = Some(Nat::from(MAX_SATOSHIS + 1));
        for refused in [
            request("not an address", 1_000),
            request(&mainnet, 1_000),
            // P2WPKH outputs below 294 satoshis are dust.
            request(&to, 293),
            request(&to, 0),
            request(&to, MAX_SATOSHIS + 1),
            with_memo,
            huge_fee,
        ] {
            assert!(
                matches!(
                    check(&refused, REGTEST),
                    Err(WalletError::InvalidArgument(_))
                ),
                "{refused:?}"
            );
        }
        assert!(check(&request(&to, 294), REGTEST).is_ok());
    }

    #[test]
    fn the_fee_rate_is_the_median_and_at_least_one_satoshi_per_vbyte() {
        assert_eq!(fee_rate(&[]), DEFAULT_FEE_RATE);
        assert_eq!(fee_rate(&[1_000, 5_000, 9_000]), 5_000);
        let percentiles: Vec<u64> = (0..101).map(|p| p * 100).collect();
        assert_eq!(fee_rate(&percentiles), 5_000);
        assert_eq!(fee_rate(&[10, 20, 30]), MIN_FEE_RATE);
    }

    #[test]
    fn a_plan_spends_the_largest_outputs_and_returns_the_change() {
        let (to, change) = (friend().script_pubkey(), own().script_pubkey());
        let coins = vec![coin(1, 10_000), coin(2, 50_000), coin(3, 30_000)];
        let plan = plan(coins, 60_000, &to, &change, Fee::Rate(2_000)).unwrap();
        assert_eq!(plan.inputs, vec![coin(2, 50_000), coin(3, 30_000)]);
        assert_eq!(plan.fee, 2 * vsize(2, &[&to, &change]));
        assert_eq!(plan.change, 80_000 - 60_000 - plan.fee);
    }

    #[test]
    fn change_too_small_to_relay_goes_to_the_fee() {
        let (to, change) = (friend().script_pubkey(), own().script_pubkey());
        let fee = vsize(1, &[&to, &change]);
        // 293 satoshis of change would be dust.
        let plan = plan(
            vec![coin(1, 10_000)],
            10_000 - fee - 293,
            &to,
            &change,
            Fee::Rate(1_000),
        )
        .unwrap();
        assert_eq!(plan.change, 0);
        assert_eq!(plan.fee, fee + 293);
    }

    #[test]
    fn a_fixed_fee_is_paid_as_given_and_never_below_one_satoshi_per_vbyte() {
        let (to, change) = (friend().script_pubkey(), own().script_pubkey());
        let plan = plan(
            vec![coin(1, 100_000)],
            50_000,
            &to,
            &change,
            Fee::Total(5_000),
        )
        .unwrap();
        assert_eq!((plan.fee, plan.change), (5_000, 45_000));
        let error = super::plan(
            vec![coin(1, 100_000)],
            50_000,
            &to,
            &change,
            Fee::Total(100),
        )
        .unwrap_err();
        assert!(error.contains("below 1 satoshi per vbyte"), "{error}");
    }

    #[test]
    fn a_plan_says_what_the_account_can_spend_and_spends_at_most_32_outputs() {
        let (to, change) = (friend().script_pubkey(), own().script_pubkey());
        let error = plan(vec![coin(1, 1_000)], 5_000, &to, &change, Fee::Rate(1_000)).unwrap_err();
        assert_eq!(
            error,
            "the account can spend 1000 satoshis, less than the amount and the fee"
        );
        let many: Vec<Coin> = (0..40).map(|seed| coin(seed, 1_000)).collect();
        let error = plan(many.clone(), 35_000, &to, &change, Fee::Rate(1_000)).unwrap_err();
        assert!(error.contains("more than 32 outputs"), "{error}");
        let plan = plan(many, 20_000, &to, &change, Fee::Rate(1_000)).unwrap();
        assert!(plan.inputs.len() <= MAX_INPUTS);
    }

    /// Signs as the management canister would: `r || s` over each sighash.
    fn sign_all(hashes: &[[u8; 32]], seed: u8, high_s: bool) -> Vec<Vec<u8>> {
        let secp = Secp256k1::signing_only();
        hashes
            .iter()
            .map(|hash| {
                let mut compact = secp
                    .sign_ecdsa(&Message::from_digest(*hash), &secret(seed))
                    .serialize_compact();
                if high_s {
                    // s' = n - s also verifies, and the network refuses it.
                    let s = negate_s(&compact[32..]);
                    compact[32..].copy_from_slice(&s);
                }
                compact.to_vec()
            })
            .collect()
    }

    fn negate_s(s: &[u8]) -> [u8; 32] {
        const N: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c,
            0xd0, 0x36, 0x41, 0x41,
        ];
        let mut out = [0u8; 32];
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let mut d = i16::from(N[i]) - i16::from(s[i]) - borrow;
            borrow = i16::from(d < 0);
            if d < 0 {
                d += 256;
            }
            out[i] = d as u8;
        }
        out
    }

    #[test]
    fn a_signed_transaction_keeps_its_id_and_fits_its_estimated_size() {
        let (to, change) = (friend().script_pubkey(), own().script_pubkey());
        let coins = vec![coin(1, 40_000), coin(2, 30_000), coin(3, 20_000)];
        let plan = plan(coins, 75_000, &to, &change, Fee::Rate(1_000)).unwrap();
        let tx = unsigned(&plan, 75_000, &to, &change);
        assert_eq!(tx.output[0].value.to_sat(), 75_000);
        let values: Vec<u64> = plan.inputs.iter().map(|coin| coin.value).collect();
        let hashes = sighashes(&tx, &values, &change).unwrap();
        let id = txid(&tx);

        // Each signature is normalized to low s, as the network requires.
        let signed = signed(tx.clone(), &hashes, &sign_all(&hashes, 1, true), &public(1)).unwrap();
        assert_eq!(txid(&signed), id);
        assert!(signed.vsize() as u64 <= vsize(plan.inputs.len(), &[&to, &change]));
        let fee: u64 = values.iter().sum::<u64>()
            - signed
                .output
                .iter()
                .map(|out| out.value.to_sat())
                .sum::<u64>();
        assert_eq!(fee, plan.fee);
        assert!(fee >= signed.vsize() as u64);
        assert_eq!(from_bytes(&to_bytes(&signed)).unwrap(), signed);
        assert_eq!(
            spent_keys(&signed),
            plan.inputs.iter().map(Coin::key).collect::<Vec<_>>()
        );

        // A signature by another key, or over another value, does not verify.
        assert!(
            super::signed(
                tx.clone(),
                &hashes,
                &sign_all(&hashes, 2, false),
                &public(1)
            )
            .is_err()
        );
        let mut other = values.clone();
        other[0] += 1;
        let wrong = sighashes(&tx, &other, &change).unwrap();
        assert!(super::signed(tx, &hashes, &sign_all(&wrong, 1, false), &public(1)).is_err());
    }
}
