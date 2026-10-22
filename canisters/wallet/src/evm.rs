//! EVM accounts and transactions: an account's address, what `propose`
//! checks in a transfer of a chain's native token, the transaction it
//! signs, its fees, and its nonce. Pure: `keys` and `evm_rpc` make the
//! calls and `actions` records how they ended.
//!
//! An account's address is the last 20 bytes of the Keccak-256 hash of its
//! threshold ECDSA key, uncompressed, so one account receives on every
//! chain. A transfer is one EIP-1559 transaction with no data. The chain id
//! is part of what it signs, so it cannot be replayed on another chain.
//!
//! A chain runs an account's transactions in the order of their nonces,
//! each nonce once, and only the wallet signs for its accounts. So the
//! wallet gives a transaction the next nonce only once the chain has used
//! the nonce of every transaction it built before from that account, and a
//! transaction it may have sent holds its nonce until the chain uses it:
//! then that transaction ran, since no other has its nonce.

use crate::{BitcoinNetwork, TransferRequest, WalletError, accounts::hex};
use candid::Nat;
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId, Signature as EcdsaSignature};
use secp256k1::{Message, PublicKey, Secp256k1};

pub const ETHEREUM: u64 = 1;
pub const SEPOLIA: u64 = 11_155_111;
pub const ARBITRUM_ONE: u64 = 42_161;
pub const BASE: u64 = 8_453;
pub const OPTIMISM: u64 = 10;

/// The chains the EVM RPC canister has providers of its own for, the only
/// ones the wallet holds: no single provider is trusted, and there is no
/// URL to set.
pub const CHAINS: [u64; 5] = [ETHEREUM, SEPOLIA, ARBITRUM_ONE, BASE, OPTIMISM];

/// The chains a wallet on `network` holds. A wallet on a test network signs
/// with the subnet's test key, which is for tests, so it holds Sepolia
/// alone: a test key never controls real tokens.
pub fn chains(network: BitcoinNetwork) -> &'static [u64] {
    match network {
        BitcoinNetwork::Mainnet => &CHAINS,
        BitcoinNetwork::Testnet | BitcoinNetwork::Regtest => &[SEPOLIA],
    }
}

/// Whether a wallet on `network` holds `chain`.
pub fn holds(network: BitcoinNetwork, chain: u64) -> bool {
    chains(network).contains(&chain)
}

/// The chain a wallet on `network` tracks from the start.
pub fn first_chain(network: BitcoinNetwork) -> u64 {
    chains(network)[0]
}

/// The gas of a transfer to an account with no code.
pub const TRANSFER_GAS: u64 = 21_000;
/// At most this much gas is bought for one transfer.
pub const MAX_GAS: u64 = 10_000_000;
/// The most a transfer pays per gas, in wei: 50,000 gwei, above any base
/// fee Ethereum has had.
pub const MAX_FEE_PER_GAS: u64 = 50_000_000_000_000;
/// The least tip per gas a transfer pays on Ethereum and Sepolia, in wei,
/// as the IC's ckETH minter does: 1 gwei.
pub const MIN_L1_TIP: u64 = 1_000_000_000;
/// How many recent blocks the fees are read from.
pub const FEE_HISTORY_BLOCKS: u8 = 5;

/// Whether `chain` is an OP Stack chain, which charges a transaction beyond
/// its gas: the L1 fee of its data, and the operator's fee of its gas.
pub fn charges_beyond_gas(chain: u64) -> bool {
    matches!(chain, BASE | OPTIMISM)
}

/// An OP Stack chain's fee oracle, the `GasPriceOracle` predeploy.
pub const FEE_ORACLE: Address = [
    0x42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0f,
];
/// `getL1FeeUpperBound(uint256)`: at most what a transaction whose unsigned
/// encoding has this many bytes pays for its L1 data now.
pub const L1_FEE_UPPER_BOUND: [u8; 4] = [0xf1, 0xc7, 0xa5, 0x8b];
/// `getOperatorFee(uint256)`: what a transaction that uses this much gas
/// pays the chain's operator.
pub const OPERATOR_FEE: [u8; 4] = [0x27, 0x5a, 0xed, 0xd2];

/// The data of a call of the function `selector` with one `uint256`.
pub fn uint_call(selector: [u8; 4], arg: u64) -> String {
    let mut word = [0; 32];
    word[24..].copy_from_slice(&arg.to_be_bytes());
    format!("0x{}{}", hex(&selector), hex(&word))
}

pub type Address = [u8; 20];

pub fn keccak256(bytes: &[u8]) -> [u8; 32] {
    use tiny_keccak::Hasher;
    let mut keccak = tiny_keccak::Keccak::v256();
    keccak.update(bytes);
    let mut hash = [0; 32];
    keccak.finalize(&mut hash);
    hash
}

/// The address of a compressed SEC1 public key.
pub fn address(public_key: &[u8]) -> Result<Address, String> {
    if public_key.len() != 33 {
        return Err(format!(
            "a compressed public key has 33 bytes, not {}",
            public_key.len()
        ));
    }
    let key = PublicKey::from_slice(public_key)
        .map_err(|error| format!("not a compressed public key: {error}"))?;
    let hash = keccak256(&key.serialize_uncompressed()[1..]);
    let mut address = [0; 20];
    address.copy_from_slice(&hash[12..]);
    Ok(address)
}

/// An address as EIP-55 writes it: `0x` and its hex digits, each letter in
/// upper case where the Keccak-256 hash of the lower-case digits has a
/// nibble of 8 or more.
pub fn checksummed(address: &Address) -> String {
    let lower = hex(address);
    let hash = keccak256(lower.as_bytes());
    let digits: String = lower
        .chars()
        .enumerate()
        .map(|(at, digit)| {
            let nibble = (hash[at / 2] >> if at % 2 == 0 { 4 } else { 0 }) & 0x0f;
            if nibble >= 8 {
                digit.to_ascii_uppercase()
            } else {
                digit
            }
        })
        .collect();
    format!("0x{digits}")
}

/// Reads an address: `0x` and 40 hex digits, all of one case, or mixed as
/// EIP-55 writes them, which catches a mistyped digit.
pub fn parse_address(text: &str) -> Result<Address, String> {
    let digits = text
        .strip_prefix("0x")
        .ok_or("an EVM address starts with 0x")?;
    if digits.len() != 40 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("an EVM address is 0x and 40 hex digits".to_string());
    }
    let mut address = [0; 20];
    for (at, byte) in address.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[2 * at..2 * at + 2], 16)
            .map_err(|error| format!("not hex: {error}"))?;
    }
    let mixed = digits.bytes().any(|byte| byte.is_ascii_lowercase())
        && digits.bytes().any(|byte| byte.is_ascii_uppercase());
    if mixed && checksummed(&address)[2..] != *digits {
        return Err(
            "the address's EIP-55 checksum does not match: a digit may be mistyped".to_string(),
        );
    }
    Ok(address)
}

/// A transfer of a chain's native token as `propose` and `execute` check
/// it, in wei.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub to: Address,
    pub amount: Nat,
    /// The most it pays in fees, when the proposer fixed it.
    pub fee: Option<Nat>,
}

/// Whether `value` fits in the 256 bits of an EVM word.
fn fits_word(value: &Nat) -> bool {
    value.0.bits() <= 256
}

/// Checks a transfer from an EVM account: a recipient address other than
/// the zero address, which burns what it is sent, an amount above 0, a fee
/// cap above 0 if one is set, and no memo.
pub fn check(request: &TransferRequest) -> Result<Checked, WalletError> {
    let to = parse_address(&request.to)
        .map_err(|error| WalletError::InvalidArgument(format!("`to`: {error}")))?;
    if to == [0; 20] {
        return Err(WalletError::InvalidArgument(
            "the zero address burns what it is sent".to_string(),
        ));
    }
    if request.amount == 0u8 {
        return Err(WalletError::InvalidArgument(
            "a transfer needs an amount above 0".to_string(),
        ));
    }
    if !fits_word(&request.amount) {
        return Err(WalletError::InvalidArgument(
            "the amount is more wei than an EVM word holds".to_string(),
        ));
    }
    if let Some(fee) = &request.fee
        && (*fee == 0u8 || !fits_word(fee))
    {
        return Err(WalletError::InvalidArgument(
            "a fee cap is above 0 and fits an EVM word".to_string(),
        ));
    }
    if request.memo.is_some() {
        return Err(WalletError::InvalidArgument(
            "an EVM transfer carries no memo".to_string(),
        ));
    }
    Ok(Checked {
        to,
        amount: request.amount.clone(),
        fee: request.fee.clone(),
    })
}

/// What a transaction pays per gas, in wei, and the base fee it was priced
/// against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fees {
    pub base_fee_per_gas: Nat,
    pub max_fee_per_gas: Nat,
    pub max_priority_fee_per_gas: Nat,
}

/// The least tip per gas a transaction on `chain` pays: `MIN_L1_TIP` on
/// Ethereum and Sepolia, and nothing on the L2s.
fn least_tip(chain: u64) -> u64 {
    if matches!(chain, ETHEREUM | SEPOLIA) {
        MIN_L1_TIP
    } else {
        0
    }
}

/// Whether a transaction on `chain` with `fees` would get into a block now:
/// its fee cap pays the base fee and the least tip. One that would not
/// waits in the providers' pools holding its nonce, and every later
/// transaction from its account on the chain waits behind it.
pub fn runs_now(chain: u64, fees: &Fees) -> bool {
    fees.max_fee_per_gas >= fees.base_fee_per_gas.clone() + least_tip(chain)
}

fn median(mut values: Vec<Nat>) -> Option<Nat> {
    values.sort();
    values.get(values.len() / 2).cloned()
}

/// What one provider's `eth_feeHistory` says a new transaction pays per
/// gas: the next block's base fee, the last one it reports, and the median
/// of its blocks' median tips.
pub fn fee_estimate(base_fees: &[Nat], tips: &[Vec<Nat>]) -> Option<(Nat, Nat)> {
    let base = base_fees.last()?.clone();
    let tip =
        median(tips.iter().filter_map(|tip| tip.first().cloned()).collect()).unwrap_or_default();
    Some((base, tip))
}

/// The fees of a new transaction on `chain`, from the estimates of the
/// providers that answered. The base fee is the higher median of theirs and
/// the tip the lower, so that with two answers neither provider alone can
/// stall the transaction or raise what it pays; on Ethereum and Sepolia the
/// tip is at least `MIN_L1_TIP`. The fee cap is twice the base fee and the
/// tip, so the transaction stays includable while the base fee rises for
/// six full blocks in a row.
pub fn fees(chain: u64, estimates: &[(Nat, Nat)]) -> Result<Fees, String> {
    let mut bases: Vec<Nat> = estimates.iter().map(|(base, _)| base.clone()).collect();
    let mut tips: Vec<Nat> = estimates.iter().map(|(_, tip)| tip.clone()).collect();
    bases.sort();
    tips.sort();
    let base = bases
        .get(bases.len() / 2)
        .ok_or("no provider reported the fees")?;
    let tip = tips[(tips.len() - 1) / 2]
        .clone()
        .max(Nat::from(least_tip(chain)));
    let max_fee_per_gas = base.clone() * 2u8 + tip.clone();
    if max_fee_per_gas > MAX_FEE_PER_GAS {
        return Err(format!(
            "the chain's fees are above the {MAX_FEE_PER_GAS} wei per gas the wallet pays at most"
        ));
    }
    Ok(Fees {
        base_fee_per_gas: base.clone(),
        max_fee_per_gas,
        max_priority_fee_per_gas: tip,
    })
}

/// The fees with their cap lowered so that `gas` of it costs at most `fee`,
/// the cap the proposer fixed, and the tip no higher than the cap.
pub fn capped(fees: Fees, gas: u64, fee: &Nat) -> Fees {
    let most = fee.clone() / gas;
    let max_fee_per_gas = fees.max_fee_per_gas.min(most);
    Fees {
        base_fee_per_gas: fees.base_fee_per_gas,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas.min(max_fee_per_gas.clone()),
        max_fee_per_gas,
    }
}

/// The gas a transfer buys, from what the providers estimated it uses: the
/// median of their estimates, the higher of two. A plain transfer uses
/// exactly `TRANSFER_GAS`; anything else, such as a contract's code or an
/// Arbitrum transaction's share of its batch's L1 cost, may use more by the
/// time it runs, so it gets half again as much, up to `MAX_GAS`. Gas it
/// does not use is not paid for.
pub fn gas_limit(estimates: &[u64]) -> Result<u64, String> {
    let mut sorted = estimates.to_vec();
    sorted.sort_unstable();
    let estimate = *sorted
        .get(sorted.len() / 2)
        .ok_or("no provider estimated the gas")?;
    if estimate < TRANSFER_GAS {
        return Err(format!(
            "the providers estimate {estimate} gas, less than any transfer uses"
        ));
    }
    let gas = if estimate == TRANSFER_GAS {
        estimate
    } else {
        estimate.saturating_add(estimate.div_ceil(2))
    };
    if gas > MAX_GAS {
        return Err(format!(
            "the transfer needs {gas} gas, more than the {MAX_GAS} the wallet buys"
        ));
    }
    Ok(gas)
}

/// The nonce of a new transaction from an account that has used `count`
/// nonces on a chain, given the nonce of each transaction the wallet built
/// from it there that has not run for good, with its action: `count`, once
/// the chain has used each of theirs. Otherwise the action whose
/// transaction has to run first.
pub fn next_nonce<Id: Copy>(count: u64, built: &[(Id, u64)]) -> Result<u64, Id> {
    match built.iter().find(|(_, nonce)| *nonce >= count) {
        Some((id, _)) => Err(*id),
        None => Ok(count),
    }
}

/// The JSON-RPC quantity `0x` and the hex digits of `value`, without
/// leading zeros.
pub fn quantity(value: &Nat) -> String {
    format!("0x{}", value.0.to_str_radix(16))
}

/// Reads a JSON-RPC quantity as `multi_request` returns a result: a JSON
/// string of `0x` and at most 64 hex digits.
pub fn read_quantity(json: &str) -> Result<Nat, String> {
    let digits = json
        .trim()
        .strip_prefix("\"0x")
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|digits| {
            !digits.is_empty()
                && digits.len() <= 64
                && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| format!("not a quantity: {json}"))?;
    Ok(digits.bytes().fold(Nat::from(0u8), |value, digit| {
        let digit = (digit as char)
            .to_digit(16)
            .expect("checked as a hex digit");
        value * 16u8 + digit
    }))
}

fn minimal(bytes: &[u8]) -> &[u8] {
    let zeros = bytes.iter().take_while(|byte| **byte == 0).count();
    &bytes[zeros..]
}

/// Appends the RLP length prefix of an item of `len` bytes: `offset` is
/// 0x80 for a string, 0xc0 for a list.
fn rlp_prefix(out: &mut Vec<u8>, offset: u8, len: usize) {
    if len < 56 {
        out.push(offset + len as u8);
    } else {
        let len = (len as u64).to_be_bytes();
        let len = minimal(&len);
        out.push(offset + 55 + len.len() as u8);
        out.extend_from_slice(len);
    }
}

fn rlp_string(out: &mut Vec<u8>, bytes: &[u8]) {
    if let [byte] = bytes
        && *byte < 0x80
    {
        out.push(*byte);
        return;
    }
    rlp_prefix(out, 0x80, bytes.len());
    out.extend_from_slice(bytes);
}

/// An unsigned integer, big-endian with no leading zeros: 0 is empty.
fn rlp_uint(out: &mut Vec<u8>, big_endian: &[u8]) {
    rlp_string(out, minimal(big_endian));
}

fn rlp_nat(out: &mut Vec<u8>, value: &Nat) {
    rlp_uint(out, &value.0.to_bytes_be());
}

fn rlp_list(out: &mut Vec<u8>, items: &[u8]) {
    rlp_prefix(out, 0xc0, items.len());
    out.extend_from_slice(items);
}

/// The type byte of an EIP-1559 transaction.
const EIP1559: u8 = 0x02;

/// An EIP-1559 transfer of a chain's native token: no data, and no access
/// list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: Nat,
    pub max_fee_per_gas: Nat,
    pub gas_limit: u64,
    pub to: Address,
    pub value: Nat,
}

/// A signature as an EIP-1559 transaction carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub y_parity: bool,
    pub r: [u8; 32],
    pub s: [u8; 32],
}

impl Transaction {
    /// The transaction's fields, each RLP-encoded, in order.
    fn fields(&self) -> Vec<u8> {
        let mut fields = Vec::new();
        rlp_uint(&mut fields, &self.chain_id.to_be_bytes());
        rlp_uint(&mut fields, &self.nonce.to_be_bytes());
        rlp_nat(&mut fields, &self.max_priority_fee_per_gas);
        rlp_nat(&mut fields, &self.max_fee_per_gas);
        rlp_uint(&mut fields, &self.gas_limit.to_be_bytes());
        rlp_string(&mut fields, &self.to);
        rlp_nat(&mut fields, &self.value);
        rlp_string(&mut fields, &[]);
        rlp_list(&mut fields, &[]);
        fields
    }

    fn typed(fields: &[u8]) -> Vec<u8> {
        let mut out = vec![EIP1559];
        rlp_list(&mut out, fields);
        out
    }

    /// The length of its unsigned encoding.
    pub fn unsigned_len(&self) -> usize {
        Self::typed(&self.fields()).len()
    }

    /// What its signature signs: the Keccak-256 hash of its type and its
    /// fields.
    pub fn sighash(&self) -> [u8; 32] {
        keccak256(&Self::typed(&self.fields()))
    }

    /// The transaction with `signature`, as the network takes it.
    pub fn signed(&self, signature: &Signature) -> Vec<u8> {
        let mut fields = self.fields();
        rlp_uint(&mut fields, &[u8::from(signature.y_parity)]);
        rlp_uint(&mut fields, &signature.r);
        rlp_uint(&mut fields, &signature.s);
        Self::typed(&fields)
    }

    /// The most it pays in fees, if it uses all its gas at its fee cap.
    pub fn max_fee(&self) -> Nat {
        self.max_fee_per_gas.clone() * self.gas_limit
    }
}

/// How long after a send the wallet first sends a transaction again while
/// the chain has not used its nonce; each later wait doubles, up to a day.
/// A block takes seconds, so a transaction still waiting after minutes may
/// have left the providers' pools.
pub const FIRST_RESEND_NS: u64 = 10 * 60 * 1_000_000_000;
pub const MAX_RESEND_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

/// The wait before the send after `sends` of them.
pub fn resend_wait(sends: u32) -> u64 {
    let doublings = sends.saturating_sub(1).min(16);
    FIRST_RESEND_NS
        .saturating_mul(1 << doublings)
        .min(MAX_RESEND_NS)
}

/// A transaction's hash, the id the chain knows it by.
pub fn tx_hash(signed: &[u8]) -> [u8; 32] {
    keccak256(signed)
}

/// The `0x` and hex digits of a transaction hash.
pub fn hash_text(hash: &[u8; 32]) -> String {
    format!("0x{}", hex(hash))
}

/// The signature of `sighash` by `public_key` from the 64 bytes of `r` and
/// `s` the management canister returns: checked, with the low `s` the
/// chains require, and the parity that lets a node recover `public_key`
/// from it.
pub fn signature(
    compact: &[u8],
    sighash: &[u8; 32],
    public_key: &[u8],
) -> Result<Signature, String> {
    let secp = Secp256k1::verification_only();
    let key =
        PublicKey::from_slice(public_key).map_err(|error| format!("not a public key: {error}"))?;
    let mut signature = EcdsaSignature::from_compact(compact)
        .map_err(|error| format!("not a signature: {error}"))?;
    signature.normalize_s();
    let message = Message::from_digest(*sighash);
    secp.verify_ecdsa(&message, &signature, &key)
        .map_err(|_| "the signature does not verify with the account's key".to_string())?;
    let compact = signature.serialize_compact();
    for parity in [0, 1] {
        let id = RecoveryId::from_i32(parity).expect("0 and 1 are recovery ids");
        let recovered = RecoverableSignature::from_compact(&compact, id)
            .and_then(|recoverable| secp.recover_ecdsa(&message, &recoverable));
        if recovered == Ok(key) {
            let (mut r, mut s) = ([0; 32], [0; 32]);
            r.copy_from_slice(&compact[..32]);
            s.copy_from_slice(&compact[32..]);
            return Ok(Signature {
                y_parity: parity == 1,
                r,
                s,
            });
        }
    }
    Err("no parity recovers the account's key from the signature".to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use secp256k1::SecretKey;

    pub fn unhex(text: &str) -> Vec<u8> {
        let text = text.strip_prefix("0x").unwrap_or(text);
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    /// The key of EIP-155's example, 0x4646…46.
    pub fn secret() -> SecretKey {
        SecretKey::from_slice(&[0x46; 32]).unwrap()
    }

    pub fn public_key() -> Vec<u8> {
        secret()
            .public_key(&Secp256k1::signing_only())
            .serialize()
            .to_vec()
    }

    /// Signs as a threshold key would: 64 bytes of `r` and `s`.
    pub fn sign(hash: &[u8; 32]) -> Vec<u8> {
        Secp256k1::signing_only()
            .sign_ecdsa(&Message::from_digest(*hash), &secret())
            .serialize_compact()
            .to_vec()
    }

    fn nat(text: &str) -> Nat {
        Nat::parse(text.as_bytes()).unwrap()
    }

    /// A big-endian unsigned integer of any width.
    fn from_be(bytes: &[u8]) -> Nat {
        bytes
            .iter()
            .fold(Nat::from(0u8), |value, byte| value * 256u32 + *byte)
    }

    fn array<const N: usize>(text: &str) -> [u8; N] {
        unhex(text).try_into().unwrap()
    }

    /// Transactions ethers 6.13.4 signed with the example key, with what it
    /// serialized.
    struct Vector {
        tx: Transaction,
        sighash: &'static str,
        y_parity: bool,
        signed: &'static str,
        hash: &'static str,
    }

    fn vectors() -> Vec<Vector> {
        vec![
            Vector {
                tx: Transaction {
                    chain_id: 1,
                    nonce: 9,
                    max_priority_fee_per_gas: nat("2000000000"),
                    max_fee_per_gas: nat("100000000000"),
                    gas_limit: 21_000,
                    to: [0x35; 20],
                    value: nat("1000000000000000000"),
                },
                sighash: "d6a0cf4cd9a39c3e71eb5638a92a3a9cfee2004137b97a49aff978c9e2094ede",
                y_parity: false,
                signed: "02f8730109847735940085174876e800825208943535353535353535353535353535353535353535880de0b6b3a764000080c080a0262ae4928c1d9449769124b9ab0a22ae502cc0935e6e47ae44aa2c142a51960aa060a9a373f45bb1195d1a1693714d5a5434068c64d75ddb60d1890c776169590c",
                hash: "c72d024ce5bf622efa65e2fda845d4cc3d698a39276145ae4d8fac6a8de2469a",
            },
            Vector {
                tx: Transaction {
                    chain_id: SEPOLIA,
                    nonce: 0,
                    max_priority_fee_per_gas: nat("1"),
                    max_fee_per_gas: nat("1"),
                    gas_limit: 21_000,
                    to: array("0000000000000000000000000000000000000001"),
                    value: nat("1"),
                },
                sighash: "b395af0c3496f3ab59d235370ecb4552682c97611c13a2515dd6889dde1b8f94",
                y_parity: false,
                signed: "02f86583aa36a78001018252089400000000000000000000000000000000000000010180c080a006affd5da2fdf72af8279a53acdae5208db6d88c6d2fb81a512206196ab5322fa039308587b45689518a7157d2b9ce45849122c880bb98f162370efb1f164de4ed",
                hash: "7a78470ba95f721fd09a6d84f1ed50497f88f036a17a64d299b11efb9becfd59",
            },
            Vector {
                tx: Transaction {
                    chain_id: ARBITRUM_ONE,
                    nonce: 300,
                    max_priority_fee_per_gas: nat("0"),
                    max_fee_per_gas: nat("10000000"),
                    gas_limit: 1_234_567,
                    to: [0xab; 20],
                    // 2^255 + 7: a value of all 32 bytes.
                    value: nat(
                        "57896044618658097711785492504343953926634992332820282019728792003956564819975",
                    ),
                },
                sighash: "e4ad9ad2758b9b9c00f0b1c9b83810649edf8b126f3fc3a7a346a19fe4379808",
                y_parity: true,
                signed: "02f88a82a4b182012c80839896808312d68794ababababababababababababababababababababa0800000000000000000000000000000000000000000000000000000000000000780c001a07b9384a77df2d2a34875d013a9cc796ae0969fce4311a13a1aa06c5b8015674da070c5fe87f1bf5d74788d01cdf5c8014466235d1a030349eb77aed32550f9d1bb",
                hash: "1082494fe35ffaa0a9b517fc2a2aa9d33f99d76c9c6db11567999baf9e1d4ca8",
            },
            Vector {
                tx: Transaction {
                    chain_id: BASE,
                    nonce: 1,
                    max_priority_fee_per_gas: nat("1000"),
                    max_fee_per_gas: nat("2000"),
                    gas_limit: 21_000,
                    to: [0x12; 20],
                    // A one-byte value that RLP writes with a prefix.
                    value: nat("128"),
                },
                sighash: "867a2238d2548b794ba5e2a9fd2e4073cef33bd91f33570e4bc661ca3de9c255",
                y_parity: true,
                signed: "02f869822105018203e88207d0825208941212121212121212121212121212121212121212818080c001a0c6f357c031f0b01f256ffc0189a586e4836a652579557474e6ccaca65dbcd6e4a01688737e8c36bfc99211e1d49dc52732b6f6b58051146c3336ff79dd321d07f3",
                hash: "72880203cb8250b9d2b2ef974993c9385a020cb8e51fb0b82cdf2e26ff0ecaa1",
            },
            Vector {
                tx: Transaction {
                    chain_id: OPTIMISM,
                    nonce: 127,
                    max_priority_fee_per_gas: nat("0"),
                    max_fee_per_gas: nat("0"),
                    gas_limit: 21_000,
                    to: [0xff; 20],
                    value: nat("0"),
                },
                sighash: "c1d82eae903184c3c5d43d379c4d1169c2620dea5fc997e4743dfc6a90e60c14",
                y_parity: false,
                signed: "02f8620a7f808082520894ffffffffffffffffffffffffffffffffffffffff8080c080a0d382110faef6889db4c14ef90985725e2b49ac1aa352fc5117afa0e63e8159a8a0047d0d5d65299a1feea41840f586dc8c5817359f36a1775969678d9300a84532",
                hash: "c0b6a35405c25f2f9a7e21fda68a2b299293adcca3a8cd53ae102c32f635c536",
            },
        ]
    }

    #[test]
    fn keccak256_is_ethereums() {
        assert_eq!(
            hex(&keccak256(b"")),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn an_address_is_the_hash_of_its_key_written_with_its_checksum() {
        // EIP-155's example key.
        let address = address(&public_key()).unwrap();
        assert_eq!(
            checksummed(&address),
            "0x9d8A62f656a8d1615C1294fd71e9CFb3E4855A4F"
        );
        assert_eq!(
            hex(&public_key()),
            "024bc2a31265153f07e70e0bab08724e6b85e217f8cd628ceb62974247bb493382"
        );
        let uncompressed = secret()
            .public_key(&Secp256k1::signing_only())
            .serialize_uncompressed();
        assert!(super::address(&uncompressed).is_err());
        // A prefix no key has, and an x past the field's prime.
        assert!(super::address(&[5; 33]).is_err());
        let mut past = [0xff; 33];
        past[0] = 2;
        assert!(super::address(&past).is_err());
    }

    #[test]
    fn checksums_are_eip55s() {
        for text in [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ] {
            let address = parse_address(text).unwrap();
            assert_eq!(checksummed(&address), text);
            assert_eq!(parse_address(&text.to_lowercase()), Ok(address));
            assert_eq!(
                parse_address(&format!("0x{}", text[2..].to_uppercase())),
                Ok(address)
            );
        }
    }

    #[test]
    fn an_address_with_a_wrong_checksum_or_shape_is_refused() {
        // One letter's case changed.
        assert!(
            parse_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD")
                .unwrap_err()
                .contains("checksum")
        );
        for text in [
            "5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
            "0X5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1bea",
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed00",
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaeg",
            "0x+aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
        ] {
            assert!(parse_address(text).is_err(), "{text}");
        }
    }

    #[test]
    fn transactions_encode_and_sign_as_ethers_does() {
        for vector in vectors() {
            let sighash = vector.tx.sighash();
            assert_eq!(hex(&sighash), vector.sighash);
            let signature = signature(&sign(&sighash), &sighash, &public_key()).unwrap();
            assert_eq!(signature.y_parity, vector.y_parity, "{}", vector.hash);
            let signed = vector.tx.signed(&signature);
            assert_eq!(hex(&signed), vector.signed);
            assert_eq!(hex(&tx_hash(&signed)), vector.hash);
        }
    }

    #[test]
    fn a_high_s_is_made_low_and_its_parity_follows() {
        let order =
            nat("115792089237316195423570985008687907852837564279074904382605163141518161494337");
        for vector in vectors() {
            let sighash = vector.tx.sighash();
            let low = sign(&sighash);
            let s = from_be(&low[32..]);
            let high_s = (order.clone() - s).0.to_bytes_be();
            let mut high = low[..32].to_vec();
            high.extend(std::iter::repeat_n(0, 32 - high_s.len()));
            high.extend(high_s);
            let signature = signature(&high, &sighash, &public_key()).unwrap();
            assert_eq!(hex(&vector.tx.signed(&signature)), vector.signed);
        }
    }

    #[test]
    fn a_signature_by_another_key_or_of_another_hash_is_refused() {
        let sighash = vectors()[0].tx.sighash();
        let other = SecretKey::from_slice(&[7; 32]).unwrap();
        let by_other = Secp256k1::signing_only()
            .sign_ecdsa(&Message::from_digest(sighash), &other)
            .serialize_compact();
        assert!(signature(&by_other, &sighash, &public_key()).is_err());
        let of_other = sign(&[1; 32]);
        assert!(signature(&of_other, &sighash, &public_key()).is_err());
        assert!(signature(&[0; 64], &sighash, &public_key()).is_err());
    }

    #[test]
    fn rlp_writes_lengths_and_integers_without_leading_zeros() {
        let encode = |bytes: &[u8]| {
            let mut out = Vec::new();
            rlp_string(&mut out, bytes);
            out
        };
        assert_eq!(encode(&[]), [0x80]);
        assert_eq!(encode(&[0x00]), [0x00]);
        assert_eq!(encode(&[0x7f]), [0x7f]);
        assert_eq!(encode(&[0x80]), [0x81, 0x80]);
        let long = [0xaa; 56];
        assert_eq!(&encode(&long)[..2], [0xb8, 56]);
        let mut out = Vec::new();
        rlp_uint(&mut out, &[0, 0, 0]);
        rlp_uint(&mut out, &[0, 0, 1, 0]);
        assert_eq!(out, [0x80, 0x82, 0x01, 0x00]);
        let mut list = Vec::new();
        rlp_list(&mut list, &[0xaa; 60]);
        assert_eq!(&list[..2], [0xf8, 60]);
    }

    fn request(to: &str, amount: u64) -> TransferRequest {
        TransferRequest {
            from: 1,
            asset: Some(crate::Chain::Evm(ETHEREUM)),
            to: to.to_string(),
            amount: Nat::from(amount),
            fee: None,
            memo: None,
        }
    }

    const FRIEND: &str = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";

    #[test]
    fn a_transfer_names_an_address_and_an_amount_and_carries_no_memo() {
        let checked = check(&request(FRIEND, 5)).unwrap();
        assert_eq!(checked.to, parse_address(FRIEND).unwrap());
        assert_eq!(checked.amount, 5u8);
        let word = from_be(&[0xff; 32]);
        assert!(
            check(&TransferRequest {
                amount: word.clone(),
                fee: Some(word.clone()),
                ..request(FRIEND, 0)
            })
            .is_ok()
        );
        for refused in [
            request("0x0000000000000000000000000000000000000000", 5),
            request("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", 5),
            request(FRIEND, 0),
            TransferRequest {
                amount: word.clone() + 1u8,
                ..request(FRIEND, 0)
            },
            TransferRequest {
                fee: Some(Nat::from(0u8)),
                ..request(FRIEND, 5)
            },
            TransferRequest {
                fee: Some(word + 1u8),
                ..request(FRIEND, 5)
            },
            TransferRequest {
                memo: Some(serde_bytes::ByteBuf::from(vec![1])),
                ..request(FRIEND, 5)
            },
        ] {
            assert!(
                matches!(check(&refused), Err(WalletError::InvalidArgument(_))),
                "{refused:?}"
            );
        }
    }

    fn gwei(n: u64) -> Nat {
        Nat::from(n * 1_000_000_000)
    }

    #[test]
    fn a_providers_estimate_is_its_next_base_fee_and_median_tip() {
        let base = [gwei(10), gwei(11), gwei(12)];
        let tips = [vec![gwei(3)], vec![gwei(1)], vec![gwei(2)], vec![]];
        assert_eq!(fee_estimate(&base, &tips), Some((gwei(12), gwei(2))));
        assert_eq!(fee_estimate(&base, &[]), Some((gwei(12), Nat::from(0u8))));
        assert_eq!(fee_estimate(&[], &tips), None);
    }

    #[test]
    fn the_fee_cap_is_twice_the_base_fee_and_the_tip() {
        let fees = fees(ARBITRUM_ONE, &[(gwei(12), gwei(2))]).unwrap();
        assert_eq!(fees.max_priority_fee_per_gas, gwei(2));
        assert_eq!(fees.max_fee_per_gas, gwei(26));
        // Ethereum and Sepolia tip at least 1 gwei; the L2s as little as 0.
        for (chain, tip) in [
            (ETHEREUM, gwei(1)),
            (SEPOLIA, gwei(1)),
            (BASE, Nat::from(0u8)),
            (OPTIMISM, Nat::from(0u8)),
        ] {
            let fees = super::fees(chain, &[(gwei(1), Nat::from(0u8))]).unwrap();
            assert_eq!(fees.max_priority_fee_per_gas, tip, "{chain}");
            assert_eq!(fees.max_fee_per_gas, gwei(2) + tip);
        }
        assert!(super::fees(ETHEREUM, &[]).is_err());
        assert!(super::fees(ETHEREUM, &[(gwei(25_000), gwei(1))]).is_err());
    }

    #[test]
    fn one_provider_alone_neither_stalls_nor_overpays() {
        // Two answers: the higher base fee, the lower tip.
        let two = [(gwei(10), gwei(500)), (gwei(30), gwei(2))];
        let fees = fees(BASE, &two).unwrap();
        assert_eq!(fees.max_priority_fee_per_gas, gwei(2));
        assert_eq!(fees.max_fee_per_gas, gwei(62));
        // Three answers: the medians.
        let three = [
            (gwei(10), gwei(500)),
            (gwei(30), gwei(2)),
            (gwei(11), gwei(3)),
        ];
        let fees = super::fees(BASE, &three).unwrap();
        assert_eq!(fees.max_priority_fee_per_gas, gwei(3));
        assert_eq!(fees.max_fee_per_gas, gwei(25));
    }

    #[test]
    fn a_fixed_fee_caps_what_the_gas_costs() {
        let fees = Fees {
            base_fee_per_gas: gwei(14),
            max_fee_per_gas: gwei(30),
            max_priority_fee_per_gas: gwei(2),
        };
        let capped = capped(fees.clone(), 21_000, &(gwei(21) * 21_000u32));
        assert_eq!(capped.max_fee_per_gas, gwei(21));
        assert_eq!(capped.max_priority_fee_per_gas, gwei(2));
        assert_eq!(capped.base_fee_per_gas, gwei(14));
        let tight = super::capped(fees.clone(), 21_000, &Nat::from(21_000u32));
        assert_eq!(tight.max_fee_per_gas, 1u8);
        assert_eq!(tight.max_priority_fee_per_gas, 1u8);
        assert_eq!(super::capped(fees.clone(), 21_000, &gwei(1_000_000)), fees);
    }

    #[test]
    fn a_fee_cap_below_the_base_fee_and_the_least_tip_would_not_run_now() {
        let fees = super::fees(ETHEREUM, &[(gwei(10), gwei(2))]).unwrap();
        assert!(runs_now(ETHEREUM, &fees));
        let at = |gwei_cap: u64| capped(fees.clone(), 21_000, &(gwei(gwei_cap) * 21_000u32));
        // The base fee of 10 gwei and Ethereum's least tip of 1 gwei.
        assert!(runs_now(ETHEREUM, &at(11)));
        let short = Fees {
            max_fee_per_gas: gwei(11) - Nat::from(1u8),
            ..at(11)
        };
        assert!(!runs_now(ETHEREUM, &short));
        assert!(!runs_now(ETHEREUM, &at(10)));
        // An L2 takes no tip at all.
        let fees = super::fees(BASE, &[(gwei(10), gwei(2))]).unwrap();
        let at = |gwei_cap: u64| capped(fees.clone(), 21_000, &(gwei(gwei_cap) * 21_000u32));
        assert!(runs_now(BASE, &at(10)));
        assert!(!runs_now(BASE, &at(9)));
    }

    #[test]
    fn a_wallet_on_test_keys_holds_sepolia_alone() {
        assert_eq!(chains(BitcoinNetwork::Mainnet), CHAINS);
        assert_eq!(first_chain(BitcoinNetwork::Mainnet), ETHEREUM);
        for network in [BitcoinNetwork::Testnet, BitcoinNetwork::Regtest] {
            assert_eq!(chains(network), [SEPOLIA]);
            assert_eq!(first_chain(network), SEPOLIA);
            assert!(holds(network, SEPOLIA));
            assert!(!holds(network, ETHEREUM));
            assert!(!holds(network, BASE));
        }
        assert!(holds(BitcoinNetwork::Mainnet, OPTIMISM));
        assert!(!holds(BitcoinNetwork::Mainnet, 56));
    }

    #[test]
    fn a_plain_transfer_buys_its_gas_and_anything_else_half_again() {
        assert_eq!(gas_limit(&[21_000]), Ok(21_000));
        assert_eq!(gas_limit(&[21_000, 21_000, 90_000]), Ok(21_000));
        assert_eq!(gas_limit(&[30_000, 21_000]), Ok(45_000));
        assert_eq!(gas_limit(&[40_001]), Ok(60_002));
        assert!(gas_limit(&[]).is_err());
        assert!(gas_limit(&[20_999]).is_err());
        assert!(gas_limit(&[MAX_GAS]).is_err());
    }

    #[test]
    fn a_new_nonce_waits_until_the_chain_used_every_earlier_one() {
        assert_eq!(next_nonce::<u64>(0, &[]), Ok(0));
        assert_eq!(next_nonce(7, &[(1, 5), (2, 6)]), Ok(7));
        assert_eq!(next_nonce(7, &[(1, 6), (2, 7)]), Err(2));
        assert_eq!(next_nonce(3, &[(4, 9)]), Err(4));
    }

    #[test]
    fn the_fee_oracle_calls_are_its_functions() {
        let selector = |signature: &str| {
            let hash = keccak256(signature.as_bytes());
            [hash[0], hash[1], hash[2], hash[3]]
        };
        assert_eq!(L1_FEE_UPPER_BOUND, selector("getL1FeeUpperBound(uint256)"));
        assert_eq!(OPERATOR_FEE, selector("getOperatorFee(uint256)"));
        assert_eq!(
            checksummed(&FEE_ORACLE),
            "0x420000000000000000000000000000000000000F"
        );
        assert_eq!(
            uint_call(L1_FEE_UPPER_BOUND, 0x70),
            format!("0xf1c7a58b{}70", "0".repeat(62))
        );
        assert!(charges_beyond_gas(BASE) && charges_beyond_gas(OPTIMISM));
        assert!(!charges_beyond_gas(ETHEREUM) && !charges_beyond_gas(ARBITRUM_ONE));
        assert_eq!(vectors()[0].tx.unsigned_len(), 50);
    }

    #[test]
    fn a_transaction_is_sent_again_less_often_each_time() {
        assert_eq!(resend_wait(0), FIRST_RESEND_NS);
        assert_eq!(resend_wait(1), FIRST_RESEND_NS);
        assert_eq!(resend_wait(2), 2 * FIRST_RESEND_NS);
        assert_eq!(resend_wait(4), 8 * FIRST_RESEND_NS);
        assert_eq!(resend_wait(9), MAX_RESEND_NS);
        assert_eq!(resend_wait(u32::MAX), MAX_RESEND_NS);
    }

    #[test]
    fn quantities_are_hex_without_leading_zeros() {
        assert_eq!(quantity(&Nat::from(0u8)), "0x0");
        assert_eq!(quantity(&Nat::from(0x5208u32)), "0x5208");
        assert_eq!(read_quantity("\"0x5208\""), Ok(Nat::from(0x5208u32)));
        assert_eq!(read_quantity(" \"0x0\" "), Ok(Nat::from(0u8)));
        assert_eq!(read_quantity("\"0x00ff\""), Ok(Nat::from(255u8)));
        let max = format!("\"0x{}\"", "f".repeat(64));
        assert_eq!(read_quantity(&max).unwrap().0.bits(), 256);
        for bad in [
            "0x5208",
            "\"5208\"",
            "\"0x\"",
            "\"0xg\"",
            "null",
            "{\"result\":\"0x1\"}",
        ] {
            assert!(read_quantity(bad).is_err(), "{bad}");
        }
        assert!(read_quantity(&format!("\"0x1{}\"", "0".repeat(64))).is_err());
    }
}
