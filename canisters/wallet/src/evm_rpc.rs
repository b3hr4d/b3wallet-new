//! Calls to the EVM RPC canister, which sends one JSON-RPC request to
//! several providers of an EVM chain over HTTPS outcalls and compares their
//! answers. The wallet asks three of the canister's own providers and takes
//! an answer at least two of them agree on: no single provider is trusted,
//! and none is named by URL. Each call carries what the canister's cost
//! query for it names, and half again: the canister keeps what its outcalls
//! cost and the rest returns.
//!
//! What the providers answer is read here; what it means for a transfer is
//! `evm`'s and `sending`'s.

use crate::actions::SendReply;
use crate::calls::{self, CallFailure, call_encoded};
use crate::evm::{self, Address};
use candid::utils::ArgumentEncoder;
use candid::{CandidType, Nat, Principal, Reserved};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

/// The EVM RPC canister, `7hfb6-caaaa-aaaar-qadga-cai`.
pub const EVM_RPC: Principal = Principal::from_slice(&[0, 0, 0, 0, 2, 48, 0, 204, 1, 1]);

const WHO: &str = "the EVM RPC canister";

/// How many providers each call asks, and how many must agree.
const PROVIDERS: u8 = 3;
const AGREE: u8 = 2;

/// A provider of the canister's own, named only to say none is: the wallet
/// takes the canister's default providers of each chain. One that every
/// chain's list has, so the type fits each.
#[derive(CandidType, Clone, Copy, Debug)]
#[allow(dead_code, reason = "only its type is sent")]
enum Provider {
    PublicNode,
}

/// The chain whose providers a call asks.
#[derive(CandidType, Clone, Debug)]
enum RpcServices {
    EthMainnet(Option<Vec<Provider>>),
    EthSepolia(Option<Vec<Provider>>),
    ArbitrumOne(Option<Vec<Provider>>),
    BaseMainnet(Option<Vec<Provider>>),
    OptimismMainnet(Option<Vec<Provider>>),
}

fn services(chain: u64) -> Result<RpcServices, String> {
    Ok(match chain {
        evm::ETHEREUM => RpcServices::EthMainnet(None),
        evm::SEPOLIA => RpcServices::EthSepolia(None),
        evm::ARBITRUM_ONE => RpcServices::ArbitrumOne(None),
        evm::BASE => RpcServices::BaseMainnet(None),
        evm::OPTIMISM => RpcServices::OptimismMainnet(None),
        other => return Err(format!("the wallet holds no EVM chain {other}")),
    })
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug)]
enum ConsensusStrategy {
    Threshold { total: Option<u8>, min: u8 },
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug)]
struct RpcConfig {
    #[serde(rename = "responseSizeEstimate")]
    response_size_estimate: Option<u64>,
    #[serde(rename = "responseConsensus")]
    response_consensus: Option<ConsensusStrategy>,
}

fn config() -> Option<RpcConfig> {
    Some(RpcConfig {
        response_size_estimate: None,
        response_consensus: Some(ConsensusStrategy::Threshold {
            total: Some(PROVIDERS),
            min: AGREE,
        }),
    })
}

/// The block a read is of.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockTag {
    Latest,
    /// A block the chain will not revert.
    Finalized,
}

impl BlockTag {
    fn json(self) -> &'static str {
        match self {
            BlockTag::Latest => "latest",
            BlockTag::Finalized => "finalized",
        }
    }
}

#[derive(CandidType, Clone, Debug)]
struct GetTransactionCountArgs {
    address: String,
    block: BlockTag,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct FeeHistoryArgs {
    #[serde(rename = "blockCount")]
    block_count: Nat,
    #[serde(rename = "newestBlock")]
    newest_block: BlockTag,
    #[serde(rename = "rewardPercentiles")]
    reward_percentiles: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct FeeHistory {
    reward: Vec<Vec<Nat>>,
    #[serde(rename = "baseFeePerGas")]
    base_fee_per_gas: Vec<Nat>,
}

/// What the wallet reads of a transaction's receipt.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransactionReceipt {
    /// 1 when it ran, 0 when it reverted.
    pub status: Option<Nat>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: String,
    #[serde(rename = "gasUsed")]
    pub gas_used: Nat,
    #[serde(rename = "effectiveGasPrice")]
    pub effective_gas_price: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SendRawTransactionStatus {
    Ok(Option<String>),
    NonceTooLow,
    NonceTooHigh,
    InsufficientFunds,
}

/// The answers of the providers asked: the one at least two of them gave,
/// or each one's when they did not agree.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Multi<T> {
    Consistent(T),
    /// Each provider, which the wallet does not read, and its answer.
    Inconsistent(Vec<(Reserved, T)>),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, reason = "the EVM RPC canister's names")]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(ProviderError),
    ValidationError(ValidationError),
    HttpOutcallError(HttpOutcallError),
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

/// What one provider answered.
pub type Answer<T> = Result<T, RpcError>;

fn describe(error: &RpcError) -> String {
    match error {
        RpcError::JsonRpcError(error) => format!("error {}: {}", error.code, error.message),
        RpcError::ProviderError(error) => format!("{error:?}"),
        RpcError::ValidationError(error) => format!("{error:?}"),
        RpcError::HttpOutcallError(HttpOutcallError::IcError { code, message }) => {
            format!("the outcall failed ({code:?}): {message}")
        }
        RpcError::HttpOutcallError(HttpOutcallError::InvalidHttpJsonRpcResponse {
            status,
            parsing_error,
            ..
        }) => format!(
            "HTTP {status}{}",
            parsing_error
                .as_ref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ),
    }
}

impl<T: Debug> Multi<Answer<T>> {
    /// Every answer, as one list.
    pub fn answers(&self) -> Vec<&Answer<T>> {
        match self {
            Multi::Consistent(answer) => vec![answer],
            Multi::Inconsistent(answers) => answers.iter().map(|(_, answer)| answer).collect(),
        }
    }

    /// Why the providers gave no answer the wallet takes.
    fn why(&self) -> String {
        match self {
            Multi::Consistent(Err(error)) => format!("the providers answered {}", describe(error)),
            Multi::Consistent(Ok(value)) => format!("the providers answered {value:?}"),
            Multi::Inconsistent(answers) => {
                let answers: Vec<String> = answers
                    .iter()
                    .map(|(_, answer)| match answer {
                        Ok(value) => format!("{value:?}"),
                        Err(error) => describe(error),
                    })
                    .collect();
                format!("the providers disagree: {}", answers.join("; "))
            }
        }
    }

    /// The answer at least two providers gave.
    pub fn agreed(self) -> Result<T, String> {
        match self {
            Multi::Consistent(Ok(value)) => Ok(value),
            other => Err(other.why()),
        }
    }

    /// The values the providers answered: the one at least two of them
    /// gave, or when they disagree, each of at least two that answered one.
    /// A value two agreed on stands for both.
    pub fn values(self) -> Result<Vec<T>, String> {
        if let Multi::Inconsistent(answers) = &self {
            let count = answers.iter().filter(|(_, answer)| answer.is_ok()).count();
            if count >= usize::from(AGREE) {
                let Multi::Inconsistent(answers) = self else {
                    unreachable!("matched above");
                };
                return Ok(answers
                    .into_iter()
                    .filter_map(|(_, answer)| answer.ok())
                    .collect());
            }
        }
        self.agreed().map(|value| vec![value])
    }
}

/// The lower median: with two values the lower, so one provider alone
/// cannot raise it.
fn lower_median(mut values: Vec<Nat>) -> Option<Nat> {
    values.sort();
    let at = values.len().checked_sub(1)? / 2;
    values.get(at).cloned()
}

/// Calls `method` with `args`, carrying the cycles its cost query names and
/// half again. A failed cost query proves the call never started.
async fn request<A, R>(method: &str, args: A) -> Result<R, CallFailure>
where
    A: ArgumentEncoder,
    R: CandidType + DeserializeOwned,
{
    let args = candid::encode_args(args).expect("the arguments of a call encode");
    let cost: Answer<Nat> = call_encoded(
        EVM_RPC,
        WHO,
        &format!("{method}CyclesCost"),
        args.clone(),
        0,
    )
    .await
    .map_err(|failure| CallFailure::NotRun(calls::reason(failure)))?;
    let cost = cost.map_err(|error| {
        CallFailure::NotRun(format!(
            "{WHO} names no cost of {method}: {}",
            describe(&error)
        ))
    })?;
    let cycles = u128::try_from(cost.0 * 3u8 / 2u8)
        .map_err(|_| CallFailure::NotRun(format!("{WHO} names a cost of {method} past u128")))?;
    call_encoded(EVM_RPC, WHO, method, args, cycles).await
}

/// A JSON-RPC request of `method` with `params`, as `multi_request` takes
/// it.
fn json_request(method: &str, params: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":[{params}]}}"#)
}

/// What `multi_request` answers: each provider's result as JSON text.
async fn multi_request(
    chain: u64,
    method: &str,
    params: &str,
) -> Result<Multi<Answer<String>>, String> {
    request(
        "multi_request",
        (services(chain)?, config(), json_request(method, params)),
    )
    .await
    .map_err(calls::reason)
}

/// How many transactions `address` sent on `chain` as of `block`: the next
/// nonce. Two providers must agree.
pub async fn transaction_count(
    chain: u64,
    address: &Address,
    block: BlockTag,
) -> Result<u64, String> {
    let args = GetTransactionCountArgs {
        address: evm::checksummed(address),
        block,
    };
    let answer: Multi<Answer<Nat>> = request(
        "eth_getTransactionCount",
        (services(chain)?, config(), args),
    )
    .await
    .map_err(calls::reason)?;
    let count = answer.agreed()?;
    u64::try_from(count.0)
        .map_err(|_| "the providers count more transactions than a nonce holds".to_string())
}

/// What `address` holds on `chain` at the latest block, in wei: the lower
/// median of what the providers answered.
pub async fn balance(chain: u64, address: &Address) -> Result<Nat, String> {
    let params = format!(
        r#""{}","{}""#,
        evm::checksummed(address),
        BlockTag::Latest.json()
    );
    let values = multi_request(chain, "eth_getBalance", &params)
        .await?
        .values()?;
    let balances = values
        .iter()
        .map(|json| evm::read_quantity(json))
        .collect::<Result<Vec<_>, _>>()?;
    lower_median(balances).ok_or_else(|| "no provider answered a balance".to_string())
}

/// What each provider that answered estimates a transfer of `value` from
/// `from` to `to` uses in gas.
pub async fn estimate_gas(
    chain: u64,
    from: &Address,
    to: &Address,
    value: &Nat,
) -> Result<Vec<u64>, String> {
    let params = format!(
        r#"{{"from":"{}","to":"{}","value":"{}"}}"#,
        evm::checksummed(from),
        evm::checksummed(to),
        evm::quantity(value)
    );
    let values = multi_request(chain, "eth_estimateGas", &params)
        .await?
        .values()?;
    values
        .iter()
        .map(|json| {
            let gas = evm::read_quantity(json)?;
            u64::try_from(gas.0).map_err(|_| format!("a provider estimates {json} gas"))
        })
        .collect()
}

/// What each provider that answered estimates a new transaction on `chain`
/// pays per gas (see `evm::fee_estimate`), from the median tips of its
/// latest `evm::FEE_HISTORY_BLOCKS` blocks.
pub async fn fee_estimates(chain: u64) -> Result<Vec<(Nat, Nat)>, String> {
    let args = FeeHistoryArgs {
        block_count: Nat::from(evm::FEE_HISTORY_BLOCKS),
        newest_block: BlockTag::Latest,
        reward_percentiles: Some(vec![50]),
    };
    let answer: Multi<Answer<FeeHistory>> =
        request("eth_feeHistory", (services(chain)?, config(), args))
            .await
            .map_err(calls::reason)?;
    answer
        .values()?
        .iter()
        .map(|history| {
            evm::fee_estimate(&history.base_fee_per_gas, &history.reward)
                .ok_or_else(|| "a provider's fee history has no base fee".to_string())
        })
        .collect()
}

/// What a call of `data` to the contract `to` returns at the latest block,
/// as an unsigned integer: the higher median of the providers' answers, so
/// one provider alone cannot lower it.
async fn call_uint(chain: u64, to: &Address, data: &str) -> Result<Nat, String> {
    let params = format!(
        r#"{{"to":"{}","data":"{data}"}},"{}""#,
        evm::checksummed(to),
        BlockTag::Latest.json()
    );
    let mut values = multi_request(chain, "eth_call", &params)
        .await?
        .values()?
        .iter()
        .map(|json| evm::read_quantity(json))
        .collect::<Result<Vec<_>, _>>()?;
    values.sort();
    values
        .get(values.len() / 2)
        .cloned()
        .ok_or_else(|| "no provider answered the call".to_string())
}

/// The most a transaction whose unsigned encoding has `unsigned_len` bytes
/// and that buys `gas` pays an OP Stack chain beyond its gas, as the
/// chain's fee oracle names it now: the upper bound of its L1 data fee, and
/// the operator's fee of all its gas.
pub async fn beyond_gas(chain: u64, unsigned_len: usize, gas: u64) -> Result<Nat, String> {
    let size = u64::try_from(unsigned_len).expect("a transaction's length fits u64");
    let l1 = call_uint(
        chain,
        &evm::FEE_ORACLE,
        &evm::uint_call(evm::L1_FEE_UPPER_BOUND, size),
    )
    .await?;
    let operator = call_uint(
        chain,
        &evm::FEE_ORACLE,
        &evm::uint_call(evm::OPERATOR_FEE, gas),
    )
    .await?;
    Ok(l1 + operator)
}

/// The receipt of the transaction `hash` on `chain`, once it is in a block;
/// `None` while it is not. Two providers must agree.
pub async fn receipt(chain: u64, hash: &str) -> Result<Option<TransactionReceipt>, String> {
    let answer: Multi<Answer<Option<TransactionReceipt>>> = request(
        "eth_getTransactionReceipt",
        (services(chain)?, config(), hash.to_string()),
    )
    .await
    .map_err(calls::reason)?;
    answer.agreed()
}

/// Hands the signed transaction `signed` to the providers of `chain`.
pub async fn send(
    chain: u64,
    signed: &[u8],
) -> Result<Multi<Answer<SendRawTransactionStatus>>, CallFailure> {
    let services = services(chain).map_err(CallFailure::NotRun)?;
    let raw = format!("0x{}", crate::accounts::hex(signed));
    request("eth_sendRawTransaction", (services, config(), raw)).await
}

/// What a send's answer says, for a transaction `sent_before` or not. Any
/// provider taking it, or saying, once it was sent before, that its nonce is
/// used, is a send: the providers that answered otherwise saw it too, and
/// only this transaction has its nonce. No other answer proves a provider
/// did not pass it on, so only a call that never ran is no send.
pub fn read_send(
    result: Result<Multi<Answer<SendRawTransactionStatus>>, CallFailure>,
    sent_before: bool,
) -> SendReply {
    let answer = match result {
        Ok(answer) => answer,
        Err(CallFailure::NotRun(reason)) => return SendReply::NotRun(reason),
        Err(CallFailure::Unknown(reason)) => return SendReply::Unknown(reason),
    };
    let taken = answer.answers().iter().any(|answer| match answer {
        Ok(SendRawTransactionStatus::Ok(_)) => true,
        Ok(SendRawTransactionStatus::NonceTooLow) => sent_before,
        _ => false,
    });
    if taken {
        SendReply::Taken
    } else {
        SendReply::Unknown(answer.why())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_evm_rpc_canister_is_its_mainnet_id() {
        assert_eq!(EVM_RPC.to_text(), "7hfb6-caaaa-aaaar-qadga-cai");
    }

    fn http_error() -> RpcError {
        RpcError::HttpOutcallError(HttpOutcallError::IcError {
            code: RejectionCode::SysTransient,
            message: "timeout".to_string(),
        })
    }

    fn reserved() -> Reserved {
        Reserved
    }

    #[test]
    fn values_need_two_providers_to_agree_or_two_answers() {
        let agreed: Multi<Answer<u8>> = Multi::Consistent(Ok(7));
        assert_eq!(agreed.clone().agreed(), Ok(7));
        assert_eq!(agreed.values(), Ok(vec![7]));
        let failed: Multi<Answer<u8>> = Multi::Consistent(Err(http_error()));
        assert!(failed.clone().agreed().unwrap_err().contains("timeout"));
        assert!(failed.values().is_err());
        let two: Multi<Answer<u8>> = Multi::Inconsistent(vec![
            (reserved(), Ok(7)),
            (reserved(), Err(http_error())),
            (reserved(), Ok(9)),
        ]);
        assert!(two.clone().agreed().unwrap_err().contains("disagree"));
        assert_eq!(two.values(), Ok(vec![7, 9]));
        let one: Multi<Answer<u8>> =
            Multi::Inconsistent(vec![(reserved(), Ok(7)), (reserved(), Err(http_error()))]);
        assert!(one.values().is_err());
    }

    #[test]
    fn a_balance_is_the_lower_median() {
        let nat = |n: u32| Nat::from(n);
        assert_eq!(lower_median(vec![nat(5)]), Some(nat(5)));
        assert_eq!(lower_median(vec![nat(9), nat(5)]), Some(nat(5)));
        assert_eq!(lower_median(vec![nat(9), nat(1), nat(5)]), Some(nat(5)));
        assert_eq!(lower_median(Vec::new()), None);
    }

    #[test]
    fn a_json_request_names_its_method_and_params() {
        assert_eq!(
            json_request("eth_getBalance", r#""0x1","latest""#),
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_getBalance","params":["0x1","latest"]}"#
        );
    }

    fn status(status: SendRawTransactionStatus) -> Answer<SendRawTransactionStatus> {
        Ok(status)
    }

    #[test]
    fn a_send_is_taken_once_any_provider_took_it() {
        use SendRawTransactionStatus::*;
        let taken = Multi::Consistent(status(Ok(Some("0xab".to_string()))));
        assert_eq!(read_send(Result::Ok(taken), false), SendReply::Taken);
        let one = Multi::Inconsistent(vec![
            (reserved(), status(NonceTooHigh)),
            (reserved(), Err(http_error())),
            (reserved(), status(Ok(None))),
        ]);
        assert_eq!(read_send(Result::Ok(one), false), SendReply::Taken);
    }

    #[test]
    fn a_used_nonce_is_a_send_only_of_a_transaction_sent_before() {
        use SendRawTransactionStatus::*;
        let used = Multi::Consistent(status(NonceTooLow));
        assert_eq!(read_send(Result::Ok(used.clone()), true), SendReply::Taken);
        assert!(matches!(
            read_send(Result::Ok(used), false),
            SendReply::Unknown(_)
        ));
    }

    #[test]
    fn only_a_call_that_never_ran_is_no_send() {
        use SendRawTransactionStatus::*;
        for answer in [
            Multi::Consistent(status(InsufficientFunds)),
            Multi::Consistent(status(NonceTooHigh)),
            Multi::Consistent(Err(http_error())),
            Multi::Consistent(Err(RpcError::ProviderError(ProviderError::TooFewCycles {
                expected: Nat::from(2u8),
                received: Nat::from(1u8),
            }))),
            Multi::Inconsistent(vec![(reserved(), Err(http_error()))]),
        ] {
            assert!(
                matches!(
                    read_send(Result::Ok(answer.clone()), false),
                    SendReply::Unknown(_)
                ),
                "{answer:?}"
            );
        }
        assert_eq!(
            read_send(Err(CallFailure::NotRun("r".to_string())), true),
            SendReply::NotRun("r".to_string())
        );
        assert_eq!(
            read_send(Err(CallFailure::Unknown("r".to_string())), false),
            SendReply::Unknown("r".to_string())
        );
    }
}
