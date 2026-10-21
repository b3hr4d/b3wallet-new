//! The wallet's accounts: which chain, which label, and which derivation
//! index. Pure: `storage` keeps them, and the caller passes the wallet's own
//! canister id to derive an address.
//!
//! ICP and ICRC tokens are held by the wallet's own principal. Account index
//! `n` of a chain is the ICRC-1 subaccount that holds `n` as a 32-byte
//! big-endian number, so index 0 is the default subaccount and the plain
//! principal. The ICP ledger is reached through its ICRC-1 interface.
//!
//! Bitcoin is held by the threshold ECDSA key of the account's derivation
//! path (see `signer`), whose public key is stored with the account.

use crate::btc;
use crate::governance::check_label;
use crate::{
    Account, AccountHolder, AccountId, AddAccountArgs, Chain, WalletError, is_canister_id,
};
use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::{Account as LedgerAccount, Subaccount};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// The ICP ledger, `ryjl3-tyaaa-aaaaa-aaaba-cai`.
pub const ICP_LEDGER: Principal = Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 2, 1, 1]);
/// At most this many accounts, so every list of them stays small.
pub const MAX_ACCOUNTS: usize = 64;

/// An account as stored, under its id. Accounts are never deleted, so an id
/// never names another account and an index is never given out twice.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredAccount {
    pub chain: Chain,
    pub label: String,
    pub index: u32,
    pub holder: AccountHolder,
    pub added_at_ns: u64,
    pub added_by: Principal,
    /// The compressed SEC1 public key of a Bitcoin account. Absent for other
    /// chains, and from accounts stored before step 9, none of which is a
    /// Bitcoin account.
    pub public_key: Option<ByteBuf>,
}

impl StoredAccount {
    /// The ledger account that holds this account's tokens, for a wallet
    /// whose canister id is `wallet`.
    pub fn ledger_account(&self, wallet: Principal) -> LedgerAccount {
        LedgerAccount {
            owner: wallet,
            subaccount: subaccount(self.index),
        }
    }

    /// The address of a Bitcoin account.
    pub fn bitcoin_address(&self) -> Result<bitcoin::Address, WalletError> {
        let (Chain::Bitcoin(network), Some(key)) = (&self.chain, &self.public_key) else {
            return Err(WalletError::InvalidArgument(format!(
                "a {:?} account has no Bitcoin address",
                self.chain
            )));
        };
        btc::address(key, *network).map_err(WalletError::InvalidArgument)
    }

    /// The account as the interface shows it.
    pub fn view(&self, id: AccountId, wallet: Principal) -> Account {
        let address = match &self.chain {
            // A stored Bitcoin account always has its key.
            Chain::Bitcoin(_) => self
                .bitcoin_address()
                .map(|address| address.to_string())
                .unwrap_or_default(),
            // ICRC-1 text: the principal, then for a subaccount its checksum
            // and its hex without leading zeros.
            _ => self.ledger_account(wallet).to_string(),
        };
        Account {
            id,
            chain: self.chain.clone(),
            label: self.label.clone(),
            index: self.index,
            holder: self.holder.clone(),
            address,
        }
    }
}

/// The subaccount of index `index`: none for index 0, the default
/// subaccount, and otherwise `index` as a 32-byte big-endian number.
pub fn subaccount(index: u32) -> Option<Subaccount> {
    (index != 0).then(|| {
        let mut subaccount = [0; 32];
        subaccount[28..].copy_from_slice(&index.to_be_bytes());
        subaccount
    })
}

/// The ledger canister that holds a chain's tokens.
pub fn ledger(chain: &Chain) -> Result<Principal, WalletError> {
    match chain {
        Chain::Icp => Ok(ICP_LEDGER),
        Chain::Icrc(ledger) => Ok(*ledger),
        Chain::Bitcoin(_) => Err(WalletError::InvalidArgument(
            "a Bitcoin account has no ledger".to_string(),
        )),
        Chain::Evm(_) => Err(cannot_hold(chain)),
    }
}

fn cannot_hold(chain: &Chain) -> WalletError {
    WalletError::InvalidArgument(format!("this release cannot hold {chain:?} accounts yet"))
}

/// Checks a chain an account or a daily limit may name: the ICP ledger is
/// `Icp` and never `Icrc`, so its tokens have one daily limit, and an `Icrc`
/// ledger is a canister.
pub fn check_chain(chain: &Chain) -> Result<(), String> {
    match chain {
        Chain::Icrc(ledger) if *ledger == ICP_LEDGER => {
            Err("the ICP ledger is the chain Icp, not Icrc".to_string())
        }
        Chain::Icrc(ledger) if !is_canister_id(ledger) => {
            Err(format!("the ICRC ledger {ledger} is not a canister id"))
        }
        _ => Ok(()),
    }
}

/// Checks a new account against the existing ones and returns its index:
/// the next one on its chain, counted from 0.
pub fn check_new<'a>(
    args: &AddAccountArgs,
    existing: impl IntoIterator<Item = &'a StoredAccount>,
) -> Result<u32, WalletError> {
    check_label(&args.label)?;
    check_chain(&args.chain).map_err(WalletError::InvalidArgument)?;
    if let Chain::Evm(_) = args.chain {
        return Err(cannot_hold(&args.chain));
    }
    let (mut count, mut on_chain) = (0usize, 0u32);
    for account in existing {
        count += 1;
        if account.chain == args.chain && account.holder == AccountHolder::Wallet {
            on_chain += 1;
        }
    }
    if count >= MAX_ACCOUNTS {
        return Err(WalletError::PolicyViolation(format!(
            "a wallet has at most {MAX_ACCOUNTS} accounts"
        )));
    }
    Ok(on_chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BitcoinNetwork;
    use std::str::FromStr;

    fn wallet() -> Principal {
        Principal::from_text("uxrrr-q7777-77774-qaaaq-cai").unwrap()
    }

    fn ledger_x() -> Principal {
        Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap()
    }

    fn stored(chain: Chain, index: u32) -> StoredAccount {
        StoredAccount {
            chain,
            label: "savings".to_string(),
            index,
            holder: AccountHolder::Wallet,
            added_at_ns: 0,
            added_by: Principal::self_authenticating([1; 32]),
            public_key: None,
        }
    }

    fn args(chain: Chain) -> AddAccountArgs {
        AddAccountArgs {
            chain,
            label: "savings".to_string(),
        }
    }

    #[test]
    fn the_icp_ledger_is_its_mainnet_canister() {
        assert_eq!(ICP_LEDGER.to_text(), "ryjl3-tyaaa-aaaaa-aaaba-cai");
    }

    #[test]
    fn index_zero_is_the_plain_principal_and_index_n_is_subaccount_n() {
        assert_eq!(subaccount(0), None);
        let mut one = [0; 32];
        one[31] = 1;
        assert_eq!(subaccount(1), Some(one));
        let mut big = [0; 32];
        big[28..].copy_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(subaccount(0x0102_0304), Some(big));

        assert_eq!(
            stored(Chain::Icp, 0).view(7, wallet()).address,
            wallet().to_text()
        );
        let address = stored(Chain::Icp, 1).view(7, wallet()).address;
        assert!(address.starts_with(&format!("{}-", wallet())), "{address}");
        assert!(address.ends_with(".1"), "{address}");
    }

    #[test]
    fn an_address_reads_back_as_its_ledger_account() {
        for index in [0, 1, 255, 256, u32::MAX] {
            let account = stored(Chain::Icrc(ledger_x()), index);
            let address = account.view(1, wallet()).address;
            assert_eq!(
                LedgerAccount::from_str(&address),
                Ok(account.ledger_account(wallet())),
                "{address}"
            );
        }
    }

    #[test]
    fn indices_count_per_chain_from_zero() {
        let regtest = Chain::Bitcoin(BitcoinNetwork::Regtest);
        let existing = [
            stored(Chain::Icp, 0),
            stored(Chain::Icrc(ledger_x()), 0),
            stored(Chain::Icp, 1),
            stored(regtest.clone(), 0),
        ];
        assert_eq!(check_new(&args(Chain::Icp), &existing), Ok(2));
        assert_eq!(check_new(&args(Chain::Icrc(ledger_x())), &existing), Ok(1));
        assert_eq!(check_new(&args(Chain::Icrc(wallet())), &existing), Ok(0));
        assert_eq!(check_new(&args(regtest), &existing), Ok(1));
        assert_eq!(
            check_new(&args(Chain::Bitcoin(BitcoinNetwork::Mainnet)), &existing),
            Ok(0)
        );
        assert_eq!(check_new(&args(Chain::Icp), &[]), Ok(0));
    }

    #[test]
    fn a_bitcoin_account_shows_the_address_of_its_key() {
        use crate::btc::tests::{BIP173_KEY, BIP173_REGTEST, hex};
        let account = StoredAccount {
            public_key: Some(ByteBuf::from(hex(BIP173_KEY))),
            ..stored(Chain::Bitcoin(BitcoinNetwork::Regtest), 0)
        };
        assert_eq!(account.view(1, wallet()).address, BIP173_REGTEST);
        assert!(stored(Chain::Icp, 0).bitcoin_address().is_err());
    }

    #[test]
    fn refuses_other_chains_the_icp_ledger_as_icrc_and_bad_labels() {
        let refused = [
            args(Chain::Evm(1)),
            args(Chain::Icrc(ICP_LEDGER)),
            args(Chain::Icrc(Principal::self_authenticating([2; 32]))),
            args(Chain::Icrc(Principal::anonymous())),
            AddAccountArgs {
                chain: Chain::Icp,
                label: " ".to_string(),
            },
        ];
        for args in refused {
            assert!(
                matches!(check_new(&args, &[]), Err(WalletError::InvalidArgument(_))),
                "{args:?}"
            );
        }
    }

    #[test]
    fn a_wallet_has_at_most_sixty_four_accounts() {
        let full: Vec<StoredAccount> = (0..MAX_ACCOUNTS as u32)
            .map(|index| stored(Chain::Icp, index))
            .collect();
        assert!(matches!(
            check_new(&args(Chain::Icp), &full),
            Err(WalletError::PolicyViolation(_))
        ));
        assert_eq!(check_new(&args(Chain::Icp), &full[1..]), Ok(63));
    }
}
