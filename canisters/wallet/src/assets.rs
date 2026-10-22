//! What the wallet's accounts hold. An asset is named by where it lives
//! (`Chain`), and an account's kind says which assets it can hold: an
//! Internet Computer account holds ICP and ICRC-1 tokens, a Bitcoin account
//! the bitcoin of its network. Pure: `storage` keeps the tracked list.
//!
//! The tracked assets are the ones the accounts show and send. ICP is always
//! tracked; owners add and remove ICRC-1 ledgers. Bitcoin needs no tracking:
//! each Bitcoin account holds the bitcoin of its own network.

use crate::{AccountKind, Chain, WalletError, is_canister_id};
use candid::Principal;

/// The ICP ledger, `ryjl3-tyaaa-aaaaa-aaaba-cai`.
pub const ICP_LEDGER: Principal = Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 2, 1, 1]);
/// At most this many assets are tracked, ICP included: every account shows
/// each one, and a balance of each is a call.
pub const MAX_TRACKED: usize = 16;

/// The assets a new wallet tracks.
pub fn initial() -> Vec<Chain> {
    vec![Chain::Icp]
}

/// The ledger canister that holds an asset, for the assets ledgers hold.
pub fn ledger(asset: &Chain) -> Option<Principal> {
    match asset {
        Chain::Icp => Some(ICP_LEDGER),
        Chain::Icrc(ledger) => Some(*ledger),
        Chain::Bitcoin(_) | Chain::Evm(_) => None,
    }
}

/// Checks that an asset is well formed: the ICP ledger is `Icp` and never
/// `Icrc`, so its tokens have one daily limit, and an `Icrc` ledger is a
/// canister. Daily limits, budgets and transfers name only such assets.
pub fn check(asset: &Chain) -> Result<(), String> {
    match asset {
        Chain::Icrc(ledger) if *ledger == ICP_LEDGER => {
            Err("the ICP ledger is the asset Icp, not Icrc".to_string())
        }
        Chain::Icrc(ledger) if !is_canister_id(ledger) => {
            Err(format!("the ICRC-1 ledger {ledger} is not a canister id"))
        }
        _ => Ok(()),
    }
}

/// Whether an account of `kind` holds `asset`.
pub fn holds(kind: AccountKind, asset: &Chain) -> bool {
    match (kind, asset) {
        (AccountKind::InternetComputer, Chain::Icp | Chain::Icrc(_)) => true,
        (AccountKind::Bitcoin(network), Chain::Bitcoin(of)) => network == *of,
        _ => false,
    }
}

/// What an account of `kind` reads and sends when no asset is named: ICP, or
/// the bitcoin of its network.
pub fn default_of(kind: AccountKind) -> Option<Chain> {
    match kind {
        AccountKind::InternetComputer => Some(Chain::Icp),
        AccountKind::Bitcoin(network) => Some(Chain::Bitcoin(network)),
        AccountKind::Evm => None,
    }
}

/// The asset an account of `kind` reads or sends: `named`, or its default.
/// It must be one the account holds and, but for Bitcoin, one the wallet
/// tracks, so no member makes the wallet call a canister the owners did not
/// choose.
pub fn resolve(
    kind: AccountKind,
    named: Option<&Chain>,
    tracked: &[Chain],
) -> Result<Chain, WalletError> {
    let asset = match named {
        Some(asset) => asset.clone(),
        None => default_of(kind).ok_or_else(|| {
            WalletError::InvalidArgument(format!("name the asset of a {kind:?} account"))
        })?,
    };
    check(&asset).map_err(WalletError::InvalidArgument)?;
    if !holds(kind, &asset) {
        return Err(WalletError::InvalidArgument(format!(
            "a {kind:?} account does not hold {asset:?}"
        )));
    }
    if !matches!(asset, Chain::Bitcoin(_)) && !tracked.contains(&asset) {
        return Err(WalletError::InvalidArgument(format!(
            "the wallet does not track {asset:?}; an owner tracks it first"
        )));
    }
    Ok(asset)
}

/// Checks an asset an owner asks to track, against the tracked list.
pub fn check_track(asset: &Chain, tracked: &[Chain]) -> Result<(), WalletError> {
    check(asset).map_err(WalletError::InvalidArgument)?;
    match asset {
        Chain::Icrc(_) => {}
        Chain::Icp => {
            return Err(WalletError::InvalidArgument(
                "ICP is always tracked".to_string(),
            ));
        }
        Chain::Bitcoin(_) => {
            return Err(WalletError::InvalidArgument(
                "a Bitcoin account holds its network's bitcoin without tracking".to_string(),
            ));
        }
        Chain::Evm(_) => {
            return Err(WalletError::InvalidArgument(
                "this release cannot hold EVM assets yet".to_string(),
            ));
        }
    }
    if tracked.contains(asset) {
        return Err(WalletError::InvalidArgument(format!(
            "{asset:?} is already tracked"
        )));
    }
    if tracked.len() >= MAX_TRACKED {
        return Err(WalletError::PolicyViolation(format!(
            "a wallet tracks at most {MAX_TRACKED} assets"
        )));
    }
    Ok(())
}

/// Checks an asset an owner asks to stop tracking. Its tokens stay where
/// they are; tracking it again shows them.
pub fn check_untrack(asset: &Chain, tracked: &[Chain]) -> Result<(), WalletError> {
    if *asset == Chain::Icp {
        return Err(WalletError::InvalidArgument(
            "ICP is always tracked".to_string(),
        ));
    }
    if !tracked.contains(asset) {
        return Err(WalletError::NotFound(format!("{asset:?} is not tracked")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BitcoinNetwork;

    fn ledger_x() -> Principal {
        Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap()
    }

    const REGTEST: AccountKind = AccountKind::Bitcoin(BitcoinNetwork::Regtest);
    const IC: AccountKind = AccountKind::InternetComputer;

    #[test]
    fn the_icp_ledger_is_its_mainnet_canister() {
        assert_eq!(ICP_LEDGER.to_text(), "ryjl3-tyaaa-aaaaa-aaaba-cai");
        assert_eq!(ledger(&Chain::Icp), Some(ICP_LEDGER));
        assert_eq!(ledger(&Chain::Icrc(ledger_x())), Some(ledger_x()));
        assert_eq!(ledger(&Chain::Bitcoin(BitcoinNetwork::Mainnet)), None);
    }

    #[test]
    fn an_internet_computer_account_holds_ledger_tokens_and_a_bitcoin_one_its_network() {
        assert!(holds(IC, &Chain::Icp));
        assert!(holds(IC, &Chain::Icrc(ledger_x())));
        assert!(!holds(IC, &Chain::Bitcoin(BitcoinNetwork::Regtest)));
        assert!(holds(REGTEST, &Chain::Bitcoin(BitcoinNetwork::Regtest)));
        assert!(!holds(REGTEST, &Chain::Bitcoin(BitcoinNetwork::Mainnet)));
        assert!(!holds(REGTEST, &Chain::Icp));
        assert!(!holds(AccountKind::Evm, &Chain::Icp));
        assert!(!holds(IC, &Chain::Evm(1)));
    }

    #[test]
    fn an_asset_resolves_to_the_default_or_a_tracked_one_the_account_holds() {
        let tracked = vec![Chain::Icp, Chain::Icrc(ledger_x())];
        assert_eq!(resolve(IC, None, &tracked), Ok(Chain::Icp));
        assert_eq!(
            resolve(IC, Some(&Chain::Icrc(ledger_x())), &tracked),
            Ok(Chain::Icrc(ledger_x()))
        );
        assert_eq!(
            resolve(REGTEST, None, &tracked),
            Ok(Chain::Bitcoin(BitcoinNetwork::Regtest))
        );
        let other = Principal::from_text("qhbym-qaaaa-aaaaa-aaafq-cai").unwrap();
        for (kind, asset) in [
            (IC, Some(Chain::Icrc(other))),
            (IC, Some(Chain::Icrc(ICP_LEDGER))),
            (IC, Some(Chain::Bitcoin(BitcoinNetwork::Regtest))),
            (REGTEST, Some(Chain::Icp)),
            (REGTEST, Some(Chain::Bitcoin(BitcoinNetwork::Mainnet))),
            (AccountKind::Evm, None),
        ] {
            assert!(
                matches!(
                    resolve(kind, asset.as_ref(), &tracked),
                    Err(WalletError::InvalidArgument(_))
                ),
                "{kind:?} {asset:?}"
            );
        }
    }

    #[test]
    fn owners_track_icrc_ledgers_once_and_never_untrack_icp() {
        let tracked = initial();
        assert_eq!(tracked, [Chain::Icp]);
        assert_eq!(check_track(&Chain::Icrc(ledger_x()), &tracked), Ok(()));
        let refused = [
            Chain::Icp,
            Chain::Icrc(ICP_LEDGER),
            Chain::Icrc(Principal::self_authenticating([1; 32])),
            Chain::Bitcoin(BitcoinNetwork::Mainnet),
            Chain::Evm(1),
        ];
        for asset in refused {
            assert!(
                matches!(
                    check_track(&asset, &tracked),
                    Err(WalletError::InvalidArgument(_))
                ),
                "{asset:?}"
            );
        }
        let with_x = vec![Chain::Icp, Chain::Icrc(ledger_x())];
        assert!(check_track(&Chain::Icrc(ledger_x()), &with_x).is_err());
        assert_eq!(check_untrack(&Chain::Icrc(ledger_x()), &with_x), Ok(()));
        assert!(matches!(
            check_untrack(&Chain::Icrc(ledger_x()), &tracked),
            Err(WalletError::NotFound(_))
        ));
        assert!(check_untrack(&Chain::Icp, &with_x).is_err());
    }

    #[test]
    fn a_wallet_tracks_at_most_sixteen_assets() {
        let full: Vec<Chain> = std::iter::once(Chain::Icp)
            .chain(
                (1..MAX_TRACKED as u8)
                    .map(|n| Chain::Icrc(Principal::from_slice(&[0, 0, 0, 0, 0, 0, 9, n, 1, 1]))),
            )
            .collect();
        assert_eq!(full.len(), MAX_TRACKED);
        assert!(matches!(
            check_track(&Chain::Icrc(ledger_x()), &full),
            Err(WalletError::PolicyViolation(_))
        ));
        assert_eq!(
            check_track(&Chain::Icrc(ledger_x()), &full[..MAX_TRACKED - 1]),
            Ok(())
        );
    }
}
