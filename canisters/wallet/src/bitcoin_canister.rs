//! Calls to the IC's Bitcoin canister: an address's balance, the outputs it
//! can spend, the network's fee rates, and sending a transaction. Each call
//! carries the cycles the Bitcoin canister charges for it.

use crate::BitcoinNetwork;
use crate::btc::Coin;
use crate::calls::{self, call};
use crate::transfers::CallFailure;
use ic_cdk_bitcoin_canister::{
    GetBalanceRequest, GetCurrentFeePercentilesRequest, GetUtxosRequest, GetUtxosResponse, Network,
    NetworkInRequest, SendTransactionRequest, UtxosFilterInRequest, cost_get_balance,
    cost_get_current_fee_percentiles, cost_get_utxos, cost_send_transaction,
    get_bitcoin_canister_id,
};

/// At most this many pages of outputs are read, of up to 1,000 each.
pub const MAX_UTXO_PAGES: usize = 4;

const WHO: &str = "the Bitcoin canister";

fn in_request(network: BitcoinNetwork) -> NetworkInRequest {
    match network {
        BitcoinNetwork::Mainnet => NetworkInRequest::Mainnet,
        BitcoinNetwork::Testnet => NetworkInRequest::Testnet,
        BitcoinNetwork::Regtest => NetworkInRequest::Regtest,
    }
}

fn canister(network: BitcoinNetwork) -> candid::Principal {
    get_bitcoin_canister_id(Network::from(in_request(network)))
}

/// The satoshis `address` holds in outputs with at least one confirmation.
pub async fn balance(network: BitcoinNetwork, address: String) -> Result<u64, String> {
    let arg = GetBalanceRequest {
        address,
        network: in_request(network),
        min_confirmations: None,
    };
    let cycles = cost_get_balance(&arg);
    call(canister(network), WHO, "bitcoin_get_balance", arg, cycles)
        .await
        .map_err(calls::reason)
}

/// The outputs `address` can spend, with at least one confirmation, from
/// the first `MAX_UTXO_PAGES` pages.
pub async fn coins(network: BitcoinNetwork, address: String) -> Result<Vec<Coin>, String> {
    let mut coins = Vec::new();
    let mut filter = None;
    for _ in 0..MAX_UTXO_PAGES {
        let arg = GetUtxosRequest {
            address: address.clone(),
            network: in_request(network),
            filter,
        };
        let cycles = cost_get_utxos(&arg);
        let page: GetUtxosResponse = call(canister(network), WHO, "bitcoin_get_utxos", arg, cycles)
            .await
            .map_err(calls::reason)?;
        coins.extend(page.utxos.into_iter().map(|utxo| Coin {
            txid: utxo.outpoint.txid.into(),
            vout: utxo.outpoint.vout,
            value: utxo.value,
        }));
        match page.next_page {
            Some(next) => filter = Some(UtxosFilterInRequest::Page(next)),
            None => break,
        }
    }
    Ok(coins)
}

/// The fee rates of the network's recent transactions, in millisatoshi per
/// vbyte, from the lowest to the highest percentile.
pub async fn fee_percentiles(network: BitcoinNetwork) -> Result<Vec<u64>, String> {
    let arg = GetCurrentFeePercentilesRequest {
        network: in_request(network),
    };
    let cycles = cost_get_current_fee_percentiles(&arg);
    call(
        canister(network),
        WHO,
        "bitcoin_get_current_fee_percentiles",
        arg,
        cycles,
    )
    .await
    .map_err(calls::reason)
}

/// Hands a signed transaction to the Bitcoin canister, which passes it to
/// the network.
pub async fn send(network: BitcoinNetwork, transaction: Vec<u8>) -> Result<(), CallFailure> {
    let arg = SendTransactionRequest {
        transaction,
        network: in_request(network),
    };
    let cycles = cost_send_transaction(&arg);
    call(
        canister(network),
        WHO,
        "bitcoin_send_transaction",
        arg,
        cycles,
    )
    .await
}
