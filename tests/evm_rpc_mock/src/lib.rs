//! A stand-in for the EVM RPC canister in the wallet's PocketIC tests. It
//! has the deployed canister's methods the wallet calls, with their types
//! (`evm_rpc.did` is the deployed interface; a test checks each type
//! against it), charges for each call, and answers from an in-memory chain
//! of each chain id, the same for every provider. Its `mock_*` methods set
//! those chains up, mine and finalize what was sent, and make calls fail.
//!
//! A sent transaction is decoded and checked as a node would: canonical
//! RLP, the chain id, a low `s`, and its sender recovered from the
//! signature. Its nonce and its sender's balance decide whether it is
//! taken, as the canister reports them.

use candid::{CandidType, Nat};
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId, Signature};
use secp256k1::{Message, Secp256k1};
use serde::Deserialize;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use tiny_keccak::{Hasher, Keccak};

/// What each call costs, in cycles; its cost query names it.
const COST: u128 = 100_000_000;

const BASE: u64 = 8_453;
const OPTIMISM: u64 = 10;

/// The fee oracle of the OP Stack chains, and the selectors of its methods
/// the wallet calls.
const FEE_ORACLE: Address = [
    0x42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0f,
];
const L1_FEE_UPPER_BOUND: &str = "f1c7a58b";
const OPERATOR_FEE: &str = "275aedd2";

type Address = [u8; 20];

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct HttpHeader {
    pub value: String,
    pub name: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcApi {
    pub url: String,
    pub headers: Option<Vec<HttpHeader>>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug)]
pub enum EthMainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    Cloudflare,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug)]
pub enum EthSepoliaService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Sepolia,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug)]
pub enum L2MainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcServices {
    Custom {
        #[serde(rename = "chainId")]
        chain_id: u64,
        services: Vec<RpcApi>,
    },
    EthSepolia(Option<Vec<EthSepoliaService>>),
    EthMainnet(Option<Vec<EthMainnetService>>),
    ArbitrumOne(Option<Vec<L2MainnetService>>),
    BaseMainnet(Option<Vec<L2MainnetService>>),
    OptimismMainnet(Option<Vec<L2MainnetService>>),
}

impl RpcServices {
    fn chain_id(&self) -> u64 {
        match self {
            RpcServices::Custom { chain_id, .. } => *chain_id,
            RpcServices::EthSepolia(_) => 11_155_111,
            RpcServices::EthMainnet(_) => 1,
            RpcServices::ArbitrumOne(_) => 42_161,
            RpcServices::BaseMainnet(_) => BASE,
            RpcServices::OptimismMainnet(_) => OPTIMISM,
        }
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcService {
    Provider(u64),
    Custom(RpcApi),
    EthSepolia(EthSepoliaService),
    EthMainnet(EthMainnetService),
    ArbitrumOne(L2MainnetService),
    BaseMainnet(L2MainnetService),
    OptimismMainnet(L2MainnetService),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsensusStrategy {
    Equality,
    Threshold { total: Option<u8>, min: u8 },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcConfig {
    #[serde(rename = "responseSizeEstimate")]
    pub response_size_estimate: Option<u64>,
    #[serde(rename = "responseConsensus")]
    pub response_consensus: Option<ConsensusStrategy>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum BlockTag {
    Earliest,
    Safe,
    Finalized,
    Latest,
    Number(Nat),
    Pending,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct FeeHistoryArgs {
    #[serde(rename = "blockCount")]
    pub block_count: Nat,
    #[serde(rename = "newestBlock")]
    pub newest_block: BlockTag,
    #[serde(rename = "rewardPercentiles")]
    pub reward_percentiles: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct FeeHistory {
    pub reward: Vec<Vec<Nat>>,
    #[serde(rename = "gasUsedRatio")]
    pub gas_used_ratio: Vec<f64>,
    #[serde(rename = "oldestBlock")]
    pub oldest_block: Nat,
    #[serde(rename = "baseFeePerGas")]
    pub base_fee_per_gas: Vec<Nat>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetTransactionCountArgs {
    pub address: String,
    pub block: BlockTag,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<Nat>,
    pub data: String,
    #[serde(rename = "blockHash")]
    pub block_hash: Option<String>,
    #[serde(rename = "transactionIndex")]
    pub transaction_index: Option<Nat>,
    pub topics: Vec<String>,
    pub address: String,
    #[serde(rename = "logIndex")]
    pub log_index: Option<Nat>,
    pub removed: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransactionReceipt {
    pub to: Option<String>,
    pub status: Option<Nat>,
    pub root: Option<String>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: String,
    #[serde(rename = "blockNumber")]
    pub block_number: Nat,
    pub from: String,
    pub logs: Vec<LogEntry>,
    #[serde(rename = "blockHash")]
    pub block_hash: String,
    #[serde(rename = "type")]
    pub tx_type: String,
    #[serde(rename = "transactionIndex")]
    pub transaction_index: Nat,
    #[serde(rename = "effectiveGasPrice")]
    pub effective_gas_price: Nat,
    #[serde(rename = "logsBloom")]
    pub logs_bloom: String,
    #[serde(rename = "contractAddress")]
    pub contract_address: Option<String>,
    #[serde(rename = "gasUsed")]
    pub gas_used: Nat,
    #[serde(rename = "cumulativeGasUsed")]
    pub cumulative_gas_used: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SendRawTransactionStatus {
    Ok(Option<String>),
    NonceTooLow,
    NonceTooHigh,
    InsufficientFunds,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    TooFewCycles { expected: Nat, received: Nat },
    MissingRequiredProvider,
    ProviderNotFound,
    NoPermission,
    InvalidRpcConfig(String),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    Custom(String),
    InvalidHex(String),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum HttpOutcallError {
    IcError {
        code: RejectionCode,
        message: String,
    },
    InvalidHttpJsonRpcResponse {
        status: u16,
        body: String,
        #[serde(rename = "parsingError")]
        parsing_error: Option<String>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(ProviderError),
    ValidationError(ValidationError),
    HttpOutcallError(HttpOutcallError),
}

pub type Answer<T> = Result<T, RpcError>;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum Multi<T> {
    Consistent(T),
    Inconsistent(Vec<(RpcService, T)>),
}

/// How the mock fails a method, until told otherwise.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The call traps before it does anything.
    Trap,
    /// Every provider's outcall fails, and nothing happens.
    Error,
    /// The call does what it asks, and every provider's answer is lost.
    DoneThenError,
}

/// A transaction in a chain's pool.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PooledTx {
    pub hash: String,
    pub from: String,
    pub chain_id: u64,
    pub nonce: u64,
    pub to: String,
    pub value: Nat,
    pub gas_limit: u64,
    pub max_fee_per_gas: Nat,
    pub max_priority_fee_per_gas: Nat,
}

/// A decoded transaction.
#[derive(Clone, Debug)]
struct Tx {
    chain_id: u64,
    nonce: u64,
    tip: u128,
    max_fee: u128,
    gas_limit: u64,
    to: Address,
    value: u128,
}

#[derive(Clone, Debug)]
struct Pooled {
    hash: String,
    from: Address,
    tx: Tx,
}

struct Chain {
    id: u64,
    balances: BTreeMap<Address, u128>,
    /// The nonces each account used in the latest block, and in the
    /// finalized one.
    counts: BTreeMap<Address, u64>,
    finalized: BTreeMap<Address, u64>,
    pool: Vec<Pooled>,
    receipts: BTreeMap<String, TransactionReceipt>,
    /// Whether its providers answer no receipt, as ones that have not caught
    /// up do.
    hides_receipts: bool,
    /// Each signed transaction a send carried, in hex.
    sends: Vec<String>,
    blocks: u64,
    base_fee: u128,
    tip: u128,
    /// What `eth_estimateGas` answers, and what a transaction uses of its
    /// gas limit.
    gas: u64,
    /// Whose transfers revert once mined.
    reverts: BTreeSet<Address>,
    /// What an OP Stack chain charges beyond gas: its fee oracle names
    /// these as the upper bound of the L1 fee and as the operator's fee.
    l1_fee: u128,
    operator_fee: u128,
}

impl Chain {
    fn new(id: u64) -> Chain {
        Chain {
            id,
            balances: BTreeMap::new(),
            counts: BTreeMap::new(),
            finalized: BTreeMap::new(),
            pool: Vec::new(),
            receipts: BTreeMap::new(),
            hides_receipts: false,
            sends: Vec::new(),
            blocks: 100,
            base_fee: 10_000_000_000,
            tip: 2_000_000_000,
            gas: 21_000,
            reverts: BTreeSet::new(),
            l1_fee: 0,
            operator_fee: 0,
        }
    }

    fn count(&self, address: &Address) -> u64 {
        self.counts.get(address).copied().unwrap_or(0)
    }

    fn balance(&self, address: &Address) -> u128 {
        self.balances.get(address).copied().unwrap_or(0)
    }

    fn charges_beyond_gas(&self) -> bool {
        matches!(self.id, BASE | OPTIMISM)
    }

    /// Takes `raw`, a signed transaction, into the pool, as a node answers.
    fn send(&mut self, raw: &str) -> Answer<SendRawTransactionStatus> {
        let bytes = unhex(raw).ok_or_else(|| {
            RpcError::ValidationError(ValidationError::InvalidHex(raw.to_string()))
        })?;
        let (from, tx) = decode(&bytes).map_err(|message| json_error(-32000, message))?;
        if tx.chain_id != self.id {
            return Err(json_error(-32000, "invalid chain id".to_string()));
        }
        self.sends.push(raw.to_string());
        let hash = hash_text(&keccak(&bytes));
        if self.receipts.contains_key(&hash) {
            return Ok(SendRawTransactionStatus::NonceTooLow);
        }
        // The canister reads a node's "already known" as taken.
        if self.pool.iter().any(|pooled| pooled.hash == hash) {
            return Ok(SendRawTransactionStatus::Ok(Some(hash)));
        }
        let count = self.count(&from);
        if tx.nonce < count {
            return Ok(SendRawTransactionStatus::NonceTooLow);
        }
        let queued = self
            .pool
            .iter()
            .filter(|pooled| pooled.from == from)
            .count();
        if tx.nonce > count + queued as u64 {
            return Ok(SendRawTransactionStatus::NonceTooHigh);
        }
        if self
            .pool
            .iter()
            .any(|pooled| pooled.from == from && pooled.tx.nonce == tx.nonce)
        {
            return Err(json_error(
                -32000,
                "replacement transaction underpriced".to_string(),
            ));
        }
        let cost = u128::from(tx.gas_limit) * tx.max_fee + tx.value;
        if self.balance(&from) < cost {
            return Ok(SendRawTransactionStatus::InsufficientFunds);
        }
        self.pool.push(Pooled {
            hash: hash.clone(),
            from,
            tx,
        });
        Ok(SendRawTransactionStatus::Ok(Some(hash)))
    }

    /// Mines every pooled transaction whose nonce is its sender's next and
    /// that pays the base fee and what it spends, until none is left.
    fn mine(&mut self) -> Vec<String> {
        let mut mined = Vec::new();
        while let Some(at) = self.pool.iter().position(|pooled| {
            let tx = &pooled.tx;
            let price = self.price(tx);
            tx.nonce == self.count(&pooled.from)
                && tx.max_fee >= self.base_fee
                && self.balance(&pooled.from) >= tx.value + self.paid(tx, price)
        }) {
            let Pooled { hash, from, tx } = self.pool.remove(at);
            let price = self.price(&tx);
            let used = self.gas.min(tx.gas_limit);
            let reverted = self.reverts.contains(&tx.to);
            let spent = self.paid(&tx, price) + if reverted { 0 } else { tx.value };
            *self.balances.entry(from).or_default() -= spent;
            if !reverted {
                *self.balances.entry(tx.to).or_default() += tx.value;
            }
            *self.counts.entry(from).or_default() += 1;
            self.blocks += 1;
            self.receipts.insert(
                hash.clone(),
                TransactionReceipt {
                    to: Some(checksummed(&tx.to)),
                    status: Some(Nat::from(u8::from(!reverted))),
                    root: None,
                    transaction_hash: hash.clone(),
                    block_number: Nat::from(self.blocks),
                    from: checksummed(&from),
                    logs: Vec::new(),
                    block_hash: hash_text(&keccak(&self.blocks.to_be_bytes())),
                    tx_type: "0x2".to_string(),
                    transaction_index: Nat::from(0u8),
                    effective_gas_price: Nat::from(price),
                    logs_bloom: format!("0x{}", "0".repeat(512)),
                    contract_address: None,
                    gas_used: Nat::from(used),
                    cumulative_gas_used: Nat::from(used),
                },
            );
            mined.push(hash);
        }
        let counts = self.counts.clone();
        self.pool
            .retain(|pooled| pooled.tx.nonce >= counts.get(&pooled.from).copied().unwrap_or(0));
        mined
    }

    fn price(&self, tx: &Tx) -> u128 {
        self.base_fee + tx.tip.min(tx.max_fee.saturating_sub(self.base_fee))
    }

    /// What `tx` pays at `price` a gas, and on an OP Stack chain beyond.
    fn paid(&self, tx: &Tx, price: u128) -> u128 {
        let beyond = if self.charges_beyond_gas() {
            self.l1_fee + self.operator_fee
        } else {
            0
        };
        u128::from(self.gas.min(tx.gas_limit)) * price + beyond
    }

    /// Answers a JSON-RPC request of `multi_request` with the JSON of its
    /// result.
    fn json(&mut self, request: &str) -> Answer<String> {
        let request: Value = serde_json::from_str(request)
            .map_err(|error| json_error(-32700, format!("parse error: {error}")))?;
        if request["jsonrpc"] != "2.0" || request.get("id").is_none() {
            return Err(json_error(-32600, "invalid request".to_string()));
        }
        let params = request["params"]
            .as_array()
            .ok_or_else(|| json_error(-32602, "no params".to_string()))?;
        let param = |at: usize| params.get(at).unwrap_or(&Value::Null);
        let text = |value: &Value| -> Answer<String> {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| json_error(-32602, format!("not a string: {value}")))
        };
        let address = |value: &Value| -> Answer<Address> {
            parse_address(&text(value)?).map_err(|error| json_error(-32602, error))
        };
        let latest = |value: &Value| -> Answer<()> {
            if *value == "latest" {
                Ok(())
            } else {
                Err(json_error(
                    -32602,
                    format!("the mock reads no block {value}"),
                ))
            }
        };
        match request["method"].as_str() {
            Some("eth_getBalance") => {
                latest(param(1))?;
                Ok(quantity(self.balance(&address(param(0))?)))
            }
            Some("eth_estimateGas") => {
                let call = param(0);
                address(&call["from"])?;
                address(&call["to"])?;
                quantity_of(&text(&call["value"])?)?;
                Ok(quantity(u128::from(self.gas)))
            }
            Some("eth_call") => {
                let call = param(0);
                latest(param(1))?;
                if !self.charges_beyond_gas() || address(&call["to"])? != FEE_ORACLE {
                    return Err(json_error(3, "execution reverted".to_string()));
                }
                let data = text(&call["data"])?;
                let data = data
                    .strip_prefix("0x")
                    .filter(|data| data.len() == 8 + 64)
                    .ok_or_else(|| json_error(-32602, format!("bad call data {data}")))?;
                let (selector, argument) = data.split_at(8);
                let value = match selector {
                    L1_FEE_UPPER_BOUND => self.l1_fee,
                    OPERATOR_FEE => self.operator_fee,
                    _ => return Err(json_error(3, "execution reverted".to_string())),
                };
                let argument =
                    u128::from_str_radix(argument.trim_start_matches('0'), 16).unwrap_or(0);
                STATE.with_borrow_mut(|state| {
                    state
                        .oracle_calls
                        .push((selector.to_string(), Nat::from(argument)))
                });
                Ok(format!("\"0x{value:064x}\""))
            }
            _ => Err(json_error(-32601, "method not found".to_string())),
        }
    }
}

#[derive(Default)]
struct State {
    chains: BTreeMap<u64, Chain>,
    failures: BTreeMap<String, Failure>,
    calls: BTreeMap<String, u64>,
    /// Each fee oracle method called, with its argument.
    oracle_calls: Vec<(String, Nat)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with_chain<R>(id: u64, f: impl FnOnce(&mut Chain) -> R) -> R {
    // `json` records oracle calls in the state too, so the chain is taken
    // out while `f` runs.
    let mut chain =
        STATE.with_borrow_mut(|state| state.chains.remove(&id).unwrap_or_else(|| Chain::new(id)));
    let result = f(&mut chain);
    STATE.with_borrow_mut(|state| state.chains.insert(id, chain));
    result
}

fn json_error(code: i64, message: String) -> RpcError {
    RpcError::JsonRpcError(JsonRpcError { code, message })
}

fn outcall_error() -> RpcError {
    RpcError::HttpOutcallError(HttpOutcallError::IcError {
        code: RejectionCode::SysTransient,
        message: "the mock's providers did not answer".to_string(),
    })
}

/// Runs `method` as the canister does for every provider: charges for it,
/// or fails it as told to.
fn answer<T>(method: &str, run: impl FnOnce() -> Answer<T>) -> Multi<Answer<T>> {
    let failure = STATE.with_borrow(|state| state.failures.get(method).copied());
    if failure == Some(Failure::Trap) {
        ic_cdk::trap(format!("the mock traps in {method}"));
    }
    let available = ic_cdk::api::msg_cycles_available();
    if available < COST {
        return Multi::Consistent(Err(RpcError::ProviderError(ProviderError::TooFewCycles {
            expected: Nat::from(COST),
            received: Nat::from(available),
        })));
    }
    ic_cdk::api::msg_cycles_accept(COST);
    STATE.with_borrow_mut(|state| *state.calls.entry(method.to_string()).or_default() += 1);
    match failure {
        Some(Failure::Error) => Multi::Consistent(Err(outcall_error())),
        Some(Failure::DoneThenError) => {
            let _ = run();
            Multi::Consistent(Err(outcall_error()))
        }
        _ => Multi::Consistent(run()),
    }
}

fn cost() -> Answer<Nat> {
    Ok(Nat::from(COST))
}

#[ic_cdk::update(name = "eth_getTransactionCount")]
fn eth_get_transaction_count(
    services: RpcServices,
    _config: Option<RpcConfig>,
    args: GetTransactionCountArgs,
) -> Multi<Answer<Nat>> {
    answer("eth_getTransactionCount", || {
        let address = parse_address(&args.address)
            .map_err(|error| RpcError::ValidationError(ValidationError::Custom(error)))?;
        with_chain(services.chain_id(), |chain| match args.block {
            BlockTag::Latest => Ok(Nat::from(chain.count(&address))),
            BlockTag::Finalized => Ok(Nat::from(
                chain.finalized.get(&address).copied().unwrap_or(0),
            )),
            other => Err(json_error(
                -32602,
                format!("the mock reads no block {other:?}"),
            )),
        })
    })
}

#[ic_cdk::query(name = "eth_getTransactionCountCyclesCost")]
fn eth_get_transaction_count_cycles_cost(
    _services: RpcServices,
    _config: Option<RpcConfig>,
    _args: GetTransactionCountArgs,
) -> Answer<Nat> {
    cost()
}

#[ic_cdk::update(name = "eth_feeHistory")]
fn eth_fee_history(
    services: RpcServices,
    _config: Option<RpcConfig>,
    args: FeeHistoryArgs,
) -> Multi<Answer<FeeHistory>> {
    answer("eth_feeHistory", || {
        if args.newest_block != BlockTag::Latest {
            return Err(json_error(
                -32602,
                "the mock reads only the latest block".to_string(),
            ));
        }
        let blocks = usize::try_from(args.block_count.0)
            .unwrap_or(usize::MAX)
            .min(1024);
        let percentiles = args.reward_percentiles.unwrap_or_default().len();
        with_chain(services.chain_id(), |chain| {
            Ok(FeeHistory {
                reward: vec![vec![Nat::from(chain.tip); percentiles]; blocks],
                gas_used_ratio: vec![0.5; blocks],
                oldest_block: Nat::from(chain.blocks.saturating_sub(blocks as u64)),
                base_fee_per_gas: vec![Nat::from(chain.base_fee); blocks + 1],
            })
        })
    })
}

#[ic_cdk::query(name = "eth_feeHistoryCyclesCost")]
fn eth_fee_history_cycles_cost(
    _services: RpcServices,
    _config: Option<RpcConfig>,
    _args: FeeHistoryArgs,
) -> Answer<Nat> {
    cost()
}

#[ic_cdk::update(name = "eth_getTransactionReceipt")]
fn eth_get_transaction_receipt(
    services: RpcServices,
    _config: Option<RpcConfig>,
    hash: String,
) -> Multi<Answer<Option<TransactionReceipt>>> {
    answer("eth_getTransactionReceipt", || {
        Ok(with_chain(services.chain_id(), |chain| {
            chain
                .receipts
                .get(&hash.to_lowercase())
                .filter(|_| !chain.hides_receipts)
                .cloned()
        }))
    })
}

#[ic_cdk::query(name = "eth_getTransactionReceiptCyclesCost")]
fn eth_get_transaction_receipt_cycles_cost(
    _services: RpcServices,
    _config: Option<RpcConfig>,
    _hash: String,
) -> Answer<Nat> {
    cost()
}

#[ic_cdk::update(name = "eth_sendRawTransaction")]
fn eth_send_raw_transaction(
    services: RpcServices,
    _config: Option<RpcConfig>,
    raw: String,
) -> Multi<Answer<SendRawTransactionStatus>> {
    answer("eth_sendRawTransaction", || {
        with_chain(services.chain_id(), |chain| chain.send(&raw))
    })
}

#[ic_cdk::query(name = "eth_sendRawTransactionCyclesCost")]
fn eth_send_raw_transaction_cycles_cost(
    _services: RpcServices,
    _config: Option<RpcConfig>,
    _raw: String,
) -> Answer<Nat> {
    cost()
}

#[ic_cdk::update]
fn multi_request(
    services: RpcServices,
    _config: Option<RpcConfig>,
    json: String,
) -> Multi<Answer<String>> {
    answer("multi_request", || {
        with_chain(services.chain_id(), |chain| chain.json(&json))
    })
}

#[ic_cdk::query(name = "multi_requestCyclesCost")]
fn multi_request_cycles_cost(
    _services: RpcServices,
    _config: Option<RpcConfig>,
    _json: String,
) -> Answer<Nat> {
    cost()
}

fn address(text: &str) -> Address {
    parse_address(text).unwrap_or_else(|error| ic_cdk::trap(error))
}

fn wei(amount: Nat) -> u128 {
    u128::try_from(amount.0).unwrap_or_else(|_| ic_cdk::trap("the mock holds at most u128 wei"))
}

#[ic_cdk::update]
fn mock_set_balance(chain: u64, account: String, amount: Nat) {
    let account = address(&account);
    with_chain(chain, |chain| chain.balances.insert(account, wei(amount)));
}

#[ic_cdk::query]
fn mock_balance(chain: u64, account: String) -> Nat {
    let account = address(&account);
    Nat::from(with_chain(chain, |chain| chain.balance(&account)))
}

#[ic_cdk::query]
fn mock_count(chain: u64, account: String) -> u64 {
    let account = address(&account);
    with_chain(chain, |chain| chain.count(&account))
}

#[ic_cdk::update]
fn mock_set_fees(chain: u64, base_fee: Nat, tip: Nat) {
    with_chain(chain, |chain| {
        chain.base_fee = wei(base_fee);
        chain.tip = wei(tip);
    });
}

#[ic_cdk::update]
fn mock_set_gas(chain: u64, gas: u64) {
    with_chain(chain, |chain| chain.gas = gas);
}

#[ic_cdk::update]
fn mock_set_beyond_gas(chain: u64, l1_fee: Nat, operator_fee: Nat) {
    with_chain(chain, |chain| {
        chain.l1_fee = wei(l1_fee);
        chain.operator_fee = wei(operator_fee);
    });
}

/// Makes transfers to `to` revert once mined.
#[ic_cdk::update]
fn mock_revert(chain: u64, to: String) {
    let to = address(&to);
    with_chain(chain, |chain| chain.reverts.insert(to));
}

/// Makes the providers answer no receipt from now on, or answer again.
#[ic_cdk::update]
fn mock_hide_receipts(chain: u64, hide: bool) {
    with_chain(chain, |chain| chain.hides_receipts = hide);
}

#[ic_cdk::update]
fn mock_mine(chain: u64) -> Vec<String> {
    with_chain(chain, Chain::mine)
}

/// Finalizes every block mined so far.
#[ic_cdk::update]
fn mock_finalize(chain: u64) {
    with_chain(chain, |chain| chain.finalized = chain.counts.clone());
}

/// Drops every transaction in the pool, as nodes do with ones that wait
/// too long.
#[ic_cdk::update]
fn mock_drop_pool(chain: u64) {
    with_chain(chain, |chain| chain.pool.clear());
}

/// Fails `method` as `failure` says from now on, or no longer.
#[ic_cdk::update]
fn mock_fail(method: String, failure: Option<Failure>) {
    STATE.with_borrow_mut(|state| match failure {
        Some(failure) => state.failures.insert(method, failure),
        None => state.failures.remove(&method),
    });
}

#[ic_cdk::query]
fn mock_pool(chain: u64) -> Vec<PooledTx> {
    with_chain(chain, |chain| {
        chain
            .pool
            .iter()
            .map(|Pooled { hash, from, tx }| PooledTx {
                hash: hash.clone(),
                from: checksummed(from),
                chain_id: tx.chain_id,
                nonce: tx.nonce,
                to: checksummed(&tx.to),
                value: Nat::from(tx.value),
                gas_limit: tx.gas_limit,
                max_fee_per_gas: Nat::from(tx.max_fee),
                max_priority_fee_per_gas: Nat::from(tx.tip),
            })
            .collect()
    })
}

#[ic_cdk::query]
fn mock_sends(chain: u64) -> Vec<String> {
    with_chain(chain, |chain| chain.sends.clone())
}

#[ic_cdk::query]
fn mock_receipt(chain: u64, hash: String) -> Option<TransactionReceipt> {
    with_chain(chain, |chain| chain.receipts.get(&hash).cloned())
}

/// How many calls of `method` the mock charged for.
#[ic_cdk::query]
fn mock_calls(method: String) -> u64 {
    STATE.with_borrow(|state| state.calls.get(&method).copied().unwrap_or(0))
}

#[ic_cdk::query]
fn mock_oracle_calls() -> Vec<(String, Nat)> {
    STATE.with_borrow(|state| state.oracle_calls.clone())
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    hasher.update(bytes);
    let mut hash = [0; 32];
    hasher.finalize(&mut hash);
    hash
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_text(hash: &[u8; 32]) -> String {
    format!("0x{}", hex(hash))
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let digits = text.strip_prefix("0x")?;
    if digits.len() % 2 != 0 {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok())
        .collect()
}

fn quantity(value: u128) -> String {
    format!("\"0x{value:x}\"")
}

/// A JSON-RPC quantity: `0x` and hex digits without leading zeros.
fn quantity_of(text: &str) -> Answer<u128> {
    text.strip_prefix("0x")
        .filter(|digits| *digits == "0" || !digits.starts_with('0'))
        .and_then(|digits| u128::from_str_radix(digits, 16).ok())
        .ok_or_else(|| json_error(-32602, format!("not a quantity: {text}")))
}

/// The EIP-55 form of `address`.
fn checksummed(address: &Address) -> String {
    let digits = hex(address);
    let hash = keccak(digits.as_bytes());
    let mixed: String = digits
        .chars()
        .enumerate()
        .map(|(at, digit)| {
            let byte = hash[at / 2];
            let nibble = if at % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            if nibble >= 8 {
                digit.to_ascii_uppercase()
            } else {
                digit
            }
        })
        .collect();
    format!("0x{mixed}")
}

/// An address as a node reads it: 40 hex digits, whose checksum is right
/// when they mix cases.
fn parse_address(text: &str) -> Result<Address, String> {
    let digits = text
        .strip_prefix("0x")
        .filter(|digits| digits.len() == 40)
        .ok_or_else(|| format!("not an address: {text}"))?;
    let bytes = unhex(&format!("0x{digits}")).ok_or_else(|| format!("not an address: {text}"))?;
    let address: Address = bytes.try_into().expect("40 hex digits are 20 bytes");
    let mixed = digits.bytes().any(|digit| digit.is_ascii_uppercase())
        && digits.bytes().any(|digit| digit.is_ascii_lowercase());
    if mixed && checksummed(&address) != text {
        return Err(format!("invalid checksum: {text}"));
    }
    Ok(address)
}

/// An RLP item: whether it is a list, its payload, and the whole item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Item<'a> {
    list: bool,
    payload: &'a [u8],
    whole: &'a [u8],
}

/// The RLP item at the start of `input`, and what follows it. Refuses an
/// encoding that is not the shortest, as nodes do.
fn item(input: &[u8]) -> Result<(Item<'_>, &[u8]), String> {
    let first = *input.first().ok_or("rlp: input too short")?;
    let (list, header, len) = match first {
        0x00..=0x7f => {
            let single = Item {
                list: false,
                payload: &input[..1],
                whole: &input[..1],
            };
            return Ok((single, &input[1..]));
        }
        0x80..=0xb7 => (false, 1, usize::from(first - 0x80)),
        0xb8..=0xbf => {
            let bytes = usize::from(first - 0xb7);
            (false, 1 + bytes, long_len(input, bytes)?)
        }
        0xc0..=0xf7 => (true, 1, usize::from(first - 0xc0)),
        0xf8..=0xff => {
            let bytes = usize::from(first - 0xf7);
            (true, 1 + bytes, long_len(input, bytes)?)
        }
    };
    let end = header
        .checked_add(len)
        .filter(|end| *end <= input.len())
        .ok_or("rlp: value size exceeds available input length")?;
    let payload = &input[header..end];
    if !list && len == 1 && payload[0] < 0x80 {
        return Err("rlp: non-canonical size information".to_string());
    }
    let item = Item {
        list,
        payload,
        whole: &input[..end],
    };
    Ok((item, &input[end..]))
}

fn long_len(input: &[u8], bytes: usize) -> Result<usize, String> {
    let len = input.get(1..1 + bytes).ok_or("rlp: input too short")?;
    if len[0] == 0 {
        return Err("rlp: non-canonical size information".to_string());
    }
    let len = len
        .iter()
        .try_fold(0usize, |len, byte| {
            len.checked_mul(256).map(|len| len + usize::from(*byte))
        })
        .ok_or("rlp: size too large")?;
    if len < 56 {
        return Err("rlp: non-canonical size information".to_string());
    }
    Ok(len)
}

/// The items of a list's payload.
fn items(mut payload: &[u8]) -> Result<Vec<Item<'_>>, String> {
    let mut items = Vec::new();
    while !payload.is_empty() {
        let (item, rest) = item(payload)?;
        items.push(item);
        payload = rest;
    }
    Ok(items)
}

fn list_header(len: usize) -> Vec<u8> {
    if len < 56 {
        return vec![0xc0 + len as u8];
    }
    let bytes: Vec<u8> = len
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .collect();
    let mut header = vec![0xf7 + bytes.len() as u8];
    header.extend(bytes);
    header
}

/// An unsigned integer of at most `max` bytes, without leading zeros.
fn uint(payload: &[u8], max: usize) -> Result<&[u8], String> {
    if payload.first() == Some(&0) {
        return Err("rlp: non-canonical integer (leading zero bytes)".to_string());
    }
    if payload.len() > max {
        return Err("rlp: integer too large".to_string());
    }
    Ok(payload)
}

fn number(payload: &[u8]) -> Result<u128, String> {
    Ok(uint(payload, 16)?
        .iter()
        .fold(0, |value, byte| value << 8 | u128::from(*byte)))
}

/// Decodes a signed EIP-1559 transfer and recovers its sender.
fn decode(raw: &[u8]) -> Result<(Address, Tx), String> {
    let rest = raw
        .strip_prefix(&[0x02])
        .ok_or("transaction type not supported")?;
    let (tx, trailing) = item(rest)?;
    if !tx.list || !trailing.is_empty() {
        return Err("rlp: expected one list".to_string());
    }
    let fields = items(tx.payload)?;
    if fields.len() != 12 {
        return Err(format!("rlp: {} fields, not 12", fields.len()));
    }
    let field = |at: usize| -> Result<&[u8], String> {
        let field = fields[at];
        if field.list {
            return Err(format!("rlp: field {at} is a list"));
        }
        Ok(field.payload)
    };
    let small = |at: usize| -> Result<u64, String> {
        u64::try_from(number(field(at)?)?).map_err(|_| format!("field {at} exceeds u64"))
    };
    let to: Address = field(5)?
        .try_into()
        .map_err(|_| "a transfer names its recipient".to_string())?;
    if !field(7)?.is_empty() || fields[8].whole != [0xc0] {
        return Err("a transfer has no data and no access list".to_string());
    }
    let tx = Tx {
        chain_id: small(0)?,
        nonce: small(1)?,
        tip: number(field(2)?)?,
        max_fee: number(field(3)?)?,
        gas_limit: small(4)?,
        to,
        value: number(field(6)?)?,
    };
    let parity = small(9)?;
    let (r, s) = (uint(field(10)?, 32)?, uint(field(11)?, 32)?);
    let mut compact = [0; 64];
    compact[32 - r.len()..32].copy_from_slice(r);
    compact[64 - s.len()..].copy_from_slice(s);

    let unsigned: Vec<u8> = fields[..9]
        .iter()
        .flat_map(|field| field.whole.iter().copied())
        .collect();
    let mut message = vec![0x02];
    message.extend(list_header(unsigned.len()));
    message.extend(unsigned);
    let sighash = Message::from_digest(keccak(&message));

    let invalid = || "invalid transaction v, r, s values".to_string();
    let signature = Signature::from_compact(&compact).map_err(|_| invalid())?;
    let mut low = signature;
    low.normalize_s();
    if low != signature || parity > 1 {
        return Err(invalid());
    }
    let id = RecoveryId::from_i32(parity as i32).map_err(|_| invalid())?;
    let recoverable = RecoverableSignature::from_compact(&compact, id).map_err(|_| invalid())?;
    let key = Secp256k1::verification_only()
        .recover_ecdsa(&sighash, &recoverable)
        .map_err(|_| invalid())?;
    let hash = keccak(&key.serialize_uncompressed()[1..]);
    let from: Address = hash[12..].try_into().expect("20 bytes");
    Ok((from, tx))
}

ic_cdk::export_candid!();

#[cfg(test)]
mod tests {
    use super::*;
    use candid::types::subtype::equal;
    use candid::types::{Type, TypeInner};
    use candid_parser::utils::CandidSource;
    use std::collections::HashSet;
    use std::path::Path;

    const TARGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/evm_rpc.did");

    fn methods(env: &candid::TypeEnv, actor: &Type) -> Vec<(String, Type)> {
        let actor = env.trace_type(actor).expect("actor type");
        let service = match actor.as_ref() {
            TypeInner::Class(_, service) => service.clone(),
            _ => actor,
        };
        env.as_service(&service).expect("service type").to_vec()
    }

    /// Each method of the mock that is not one of its controls has the type
    /// of the deployed canister's method of that name, mode included.
    #[test]
    fn the_mock_has_the_deployed_canisters_types() {
        let exported = crate::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let (target_env, target_actor) = CandidSource::File(Path::new(TARGET))
            .load()
            .expect("evm_rpc.did parses");
        let target_actor = env.merge_type(target_env, target_actor.expect("target service"));
        let exported = methods(&env, &exported_actor.expect("exported service"));
        let target = methods(&env, &target_actor);
        let mut gamma = HashSet::new();
        let mut checked = 0;
        for (name, exported) in exported
            .iter()
            .filter(|(name, _)| !name.starts_with("mock_"))
        {
            let (_, target) = target
                .iter()
                .find(|(target_name, _)| target_name == name)
                .unwrap_or_else(|| panic!("the deployed canister has no {name}"));
            equal(&mut gamma, &env, exported, target)
                .unwrap_or_else(|e| panic!("{name}: {exported} differs from {target}: {e}"));
            checked += 1;
        }
        assert_eq!(checked, 10);
    }

    fn unhex(text: &str) -> Vec<u8> {
        super::unhex(text).unwrap()
    }

    /// Transfers ethers 6.13.4 signed with EIP-155's example key, 0x4646…46,
    /// whose address is 0x9d8A…5A4F.
    #[test]
    fn decodes_transfers_and_recovers_their_sender() {
        let sender = "0x9d8A62f656a8d1615C1294fd71e9CFb3E4855A4F";
        let mainnet = unhex(
            "0x02f8730109847735940085174876e800825208943535353535353535353535353535353535353535880de0b6b3a764000080c080a0262ae4928c1d9449769124b9ab0a22ae502cc0935e6e47ae44aa2c142a51960aa060a9a373f45bb1195d1a1693714d5a5434068c64d75ddb60d1890c776169590c",
        );
        let (from, tx) = decode(&mainnet).unwrap();
        assert_eq!(checksummed(&from), sender);
        assert_eq!((tx.chain_id, tx.nonce, tx.gas_limit), (1, 9, 21_000));
        assert_eq!((tx.tip, tx.max_fee), (2_000_000_000, 100_000_000_000));
        assert_eq!((tx.to, tx.value), ([0x35; 20], 1_000_000_000_000_000_000));
        assert_eq!(
            hash_text(&keccak(&mainnet)),
            "0xc72d024ce5bf622efa65e2fda845d4cc3d698a39276145ae4d8fac6a8de2469a"
        );
        let base = unhex(
            "0x02f869822105018203e88207d0825208941212121212121212121212121212121212121212818080c001a0c6f357c031f0b01f256ffc0189a586e4836a652579557474e6ccaca65dbcd6e4a01688737e8c36bfc99211e1d49dc52732b6f6b58051146c3336ff79dd321d07f3",
        );
        let (from, tx) = decode(&base).unwrap();
        assert_eq!(checksummed(&from), sender);
        assert_eq!((tx.chain_id, tx.nonce, tx.value), (8_453, 1, 0x80));

        // Another signature recovers another sender; a high `s` is refused.
        let mut changed = mainnet.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert_ne!(
            decode(&changed).map(|(from, _)| from).ok(),
            Some(parse_address(sender).unwrap())
        );
        let (tx, _) = item(&mainnet[1..]).unwrap();
        let s = items(tx.payload).unwrap()[11].payload;
        let n = unhex("0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
        let high: Vec<u8> = {
            // n - s, big-endian, 32 bytes.
            let mut out = vec![0; 32];
            let mut borrow = 0i16;
            for at in (0..32).rev() {
                let mut digit = i16::from(n[at]) - i16::from(s[at]) - borrow;
                borrow = i16::from(digit < 0);
                if digit < 0 {
                    digit += 256;
                }
                out[at] = digit as u8;
            }
            out
        };
        let mut flipped = mainnet[..mainnet.len() - 32].to_vec();
        flipped.extend(&high);
        assert_eq!(
            decode(&flipped).unwrap_err(),
            "invalid transaction v, r, s values"
        );
    }

    #[test]
    fn refuses_non_canonical_rlp() {
        assert!(item(&[0x81, 0x05]).is_err());
        assert!(item(&[0xb8, 0x05, 1, 2, 3, 4, 5]).is_err());
        assert!(uint(&[0, 1], 32).is_err());
        assert_eq!(item(&[0x05]).unwrap().0.payload, &[0x05]);
        assert!(decode(&[0x01, 0xc0]).is_err());
    }

    #[test]
    fn checksums_as_eip_55() {
        // EIP-55's own examples.
        for text in [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
            "0x52908400098527886E0F7030069857D2E4169EE7",
            "0xde709f2102306220921060314715629080e2fb77",
        ] {
            let address = parse_address(&text.to_lowercase()).unwrap();
            assert_eq!(checksummed(&address), text);
            assert_eq!(parse_address(text), Ok(address));
        }
        assert!(parse_address("0x5AAeb6053F3E94C9b9A09f33669435E7Ef1BeAed").is_err());
    }
}
