//! The wallet's accounts. An account is a kind and an index, numbered per
//! kind from 0, and one account of each kind exists from the start:
//!
//! - An Internet Computer account `n` is the ICRC-1 subaccount of the
//!   wallet's principal that holds `n` as a 32-byte big-endian number, so
//!   account 0 is the plain principal. It holds ICP and every ICRC-1 token.
//! - A Bitcoin account `n` is the threshold ECDSA key of its derivation path
//!   (see `keys`), whose public key is stored with the account, and the
//!   P2WPKH address of that key.
//! - An EVM account `n` is a key of its own the same way, and the address of
//!   that key on every EVM chain (see `evm::address`).
//!
//! Pure: `storage` keeps the accounts, and the caller passes the wallet's own
//! canister id to derive an address.

use crate::assets::ICP_LEDGER;
use crate::governance::check_label;
use crate::{
    Account, AccountHolder, AccountId, AccountKind, AddAccountArgs, BitcoinNetwork, Chain,
    WalletError,
};
use crate::{btc, evm};
use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::{Account as LedgerAccount, Subaccount};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use sha2::{Digest, Sha224};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

/// At most this many accounts, so every list of them stays small.
pub const MAX_ACCOUNTS: usize = 64;
/// What the wallet named every account it made itself before names were unique (decision 0016).
pub const OLD_DEFAULT_LABEL: &str = "Main";

/// The label of the account of `kind` the wallet makes itself, one per kind, so each is a name of its own (decision 0016).
pub fn default_label(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::InternetComputer => "ICP wallet",
        AccountKind::Bitcoin(_) => "Bitcoin wallet",
        AccountKind::Evm => "Ethereum wallet",
    }
}

/// A label as candid-profile's names rule compares names (D-045): Unicode NFKC, lowercase, runs of White_Space made
/// one space, trimmed. Two labels with the same key are the same name.
pub fn name_key(label: &str) -> String {
    let lower = label.nfkc().collect::<String>().to_lowercase();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Refuses `label` when a listed account other than `except` already has that name: names are unique, so a name stands
/// for one account (decision 0016).
pub fn check_unique<'a>(
    label: &str,
    except: Option<AccountId>,
    existing: impl IntoIterator<Item = (AccountId, &'a StoredAccount)>,
) -> Result<(), WalletError> {
    let key = name_key(label);
    let taken = existing.into_iter().any(|(id, account)| {
        Some(id) != except && account.is_listed() && name_key(&account.label) == key
    });
    if taken {
        return Err(WalletError::InvalidArgument(format!(
            "another account is already named \u{201c}{}\u{201d}: each account needs a name of its own",
            label.trim()
        )));
    }
    Ok(())
}

/// `base`, or the first of "base 2", "base 3", ... that no listed account has.
pub fn free_label<'a>(
    base: &str,
    existing: impl IntoIterator<Item = &'a StoredAccount> + Clone,
) -> String {
    let taken = |label: &str| {
        let key = name_key(label);
        existing
            .clone()
            .into_iter()
            .any(|a| a.is_listed() && name_key(&a.label) == key)
    };
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_string()
            } else {
                format!("{base} {n}")
            }
        })
        .find(|label| !taken(label))
        .unwrap_or_else(|| base.to_string())
}

/// The accounts the wallet made itself that still have the old name "Main", renamed to their kind's default label (or
/// the next free one), in id order: what an upgrade stores (decision 0016). Accounts people added or renamed keep
/// theirs, even one an owner renamed "Main".
pub fn rename_defaults(accounts: &[(AccountId, StoredAccount)]) -> Vec<(AccountId, StoredAccount)> {
    let mut all = accounts.to_vec();
    let mut renamed = Vec::new();
    for k in 0..all.len() {
        let (id, account) = &all[k];
        if account.added_by.is_some()
            || account.renamed_by.is_some()
            || !account.is_listed()
            || account.label != OLD_DEFAULT_LABEL
        {
            continue;
        }
        let Some(kind) = account.kind else { continue };
        let others: Vec<&StoredAccount> = all
            .iter()
            .filter(|(other, _)| other != id)
            .map(|(_, a)| a)
            .collect();
        let label = free_label(default_label(kind), others.iter().copied());
        let account = StoredAccount {
            label,
            ..account.clone()
        };
        renamed.push((*id, account.clone()));
        all[k].1 = account;
    }
    renamed
}

/// An account as stored, under its id. Accounts are never deleted, so an id
/// never names another account and an index is never given out twice.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredAccount {
    /// Absent only from accounts stored before accounts had kinds, which
    /// `migrate` gives one.
    pub kind: Option<AccountKind>,
    /// What an account stored before kinds held. `migrate` reads it and
    /// clears it.
    pub chain: Option<Chain>,
    pub label: String,
    pub index: u32,
    pub holder: AccountHolder,
    pub added_at_ns: u64,
    /// `None` for an account the wallet made itself.
    pub added_by: Option<Principal>,
    /// The compressed SEC1 public key of a Bitcoin or EVM account.
    pub public_key: Option<ByteBuf>,
    /// The account this one became when accounts got kinds: an ICP account
    /// and an ICRC account with the same index were one subaccount. Its id
    /// leads there, and it is no longer listed.
    pub merged_into: Option<AccountId>,
    /// The owner who last renamed it (`rename_account`), absent if nobody
    /// has. An upgrade never renames an account an owner named.
    pub renamed_by: Option<Principal>,
}

impl StoredAccount {
    /// A new account the wallet holds.
    pub fn new(
        kind: AccountKind,
        label: String,
        index: u32,
        public_key: Option<Vec<u8>>,
        added_by: Option<Principal>,
        now: u64,
    ) -> Self {
        StoredAccount {
            kind: Some(kind),
            chain: None,
            label,
            index,
            holder: AccountHolder::Wallet,
            added_at_ns: now,
            added_by,
            public_key: public_key.map(ByteBuf::from),
            merged_into: None,
            renamed_by: None,
        }
    }

    /// The account's kind; every stored account has one once `migrate` ran.
    pub fn kind(&self) -> AccountKind {
        self.kind.expect("a migrated account has a kind")
    }

    /// Whether `accounts` lists it: it was not merged into another.
    pub fn is_listed(&self) -> bool {
        self.merged_into.is_none()
    }

    /// The ledger account of an Internet Computer account, for a wallet
    /// whose canister id is `wallet`.
    pub fn ledger_account(&self, wallet: Principal) -> LedgerAccount {
        LedgerAccount {
            owner: wallet,
            subaccount: subaccount(self.index),
        }
    }

    /// The address of a Bitcoin account.
    pub fn bitcoin_address(&self) -> Result<bitcoin::Address, WalletError> {
        let (Some(AccountKind::Bitcoin(network)), Some(key)) = (self.kind, &self.public_key) else {
            return Err(WalletError::InvalidArgument(format!(
                "a {:?} account has no Bitcoin address",
                self.kind
            )));
        };
        btc::address(key, network).map_err(WalletError::InvalidArgument)
    }

    /// The address of an EVM account.
    pub fn evm_address(&self) -> Result<evm::Address, WalletError> {
        let (Some(AccountKind::Evm), Some(key)) = (self.kind, &self.public_key) else {
            return Err(WalletError::InvalidArgument(format!(
                "a {:?} account has no EVM address",
                self.kind
            )));
        };
        evm::address(key).map_err(WalletError::InvalidArgument)
    }

    /// The account as the interface shows it.
    pub fn view(&self, id: AccountId, wallet: Principal) -> Account {
        let (address, icp_account_id) = match self.kind() {
            // ICRC-1 text: the principal, then for a subaccount its checksum
            // and its hex without leading zeros.
            AccountKind::InternetComputer => {
                let account = self.ledger_account(wallet);
                let id = account_identifier(&account);
                (account.to_string(), Some(hex(&id)))
            }
            // A stored Bitcoin account always has its key.
            AccountKind::Bitcoin(_) => (
                self.bitcoin_address()
                    .map(|address| address.to_string())
                    .unwrap_or_default(),
                None,
            ),
            // A stored EVM account always has its key.
            AccountKind::Evm => (
                self.evm_address()
                    .map(|address| evm::checksummed(&address))
                    .unwrap_or_default(),
                None,
            ),
        };
        Account {
            id,
            kind: self.kind(),
            label: self.label.clone(),
            index: self.index,
            holder: self.holder.clone(),
            address,
            icp_account_id,
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

/// The ICP ledger's account identifier of an ICRC-1 account: the CRC-32 of
/// the SHA-224 hash of the domain separator, the owner and the subaccount,
/// followed by that hash.
pub fn account_identifier(account: &LedgerAccount) -> [u8; 32] {
    let hash = Sha224::new()
        .chain_update(b"\x0Aaccount-id")
        .chain_update(account.owner.as_slice())
        .chain_update(account.effective_subaccount())
        .finalize();
    let mut id = [0; 32];
    id[..4].copy_from_slice(&crc32(&hash).to_be_bytes());
    id[4..].copy_from_slice(&hash);
    id
}

/// CRC-32 as zlib and the ICP ledger compute it (IEEE 802.3, reflected).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The index a new account of `kind` gets: one past the highest the wallet
/// gave that kind, merged accounts included, so none is given twice.
pub fn next_index<'a>(
    kind: AccountKind,
    existing: impl IntoIterator<Item = &'a StoredAccount>,
) -> u32 {
    existing
        .into_iter()
        .filter(|account| account.kind == Some(kind) && account.holder == AccountHolder::Wallet)
        .map(|account| account.index + 1)
        .max()
        .unwrap_or(0)
}

/// Checks a new account against the existing ones and returns its index.
pub fn check_new<'a>(
    args: &AddAccountArgs,
    existing: impl IntoIterator<Item = &'a StoredAccount> + Clone,
) -> Result<u32, WalletError> {
    check_label(&args.label)?;
    let listed = existing
        .clone()
        .into_iter()
        .filter(|account| account.is_listed())
        .count();
    if listed >= MAX_ACCOUNTS {
        return Err(WalletError::PolicyViolation(format!(
            "a wallet has at most {MAX_ACCOUNTS} accounts"
        )));
    }
    // A new account has no id yet, so every listed account with the name refuses it.
    check_unique(
        &args.label,
        None,
        existing.clone().into_iter().map(|account| (0, account)),
    )?;
    Ok(next_index(args.kind, existing))
}

/// The kinds whose account 0 a wallet on `network` lacks: an Internet
/// Computer account, a Bitcoin account of its network, and an EVM account.
pub fn missing_defaults<'a>(
    network: BitcoinNetwork,
    existing: impl IntoIterator<Item = &'a StoredAccount> + Clone,
) -> Vec<AccountKind> {
    [
        AccountKind::InternetComputer,
        AccountKind::Bitcoin(network),
        AccountKind::Evm,
    ]
    .into_iter()
    .filter(|kind| {
        !existing.clone().into_iter().any(|account| {
            account.is_listed()
                && account.kind == Some(*kind)
                && account.index == 0
                && account.holder == AccountHolder::Wallet
        })
    })
    .collect()
}

/// What `migrate` changes in the accounts stored before accounts had kinds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migration {
    /// The accounts to store again: each one with its kind, and those merged
    /// into another with where they lead.
    pub accounts: Vec<(AccountId, StoredAccount)>,
    /// What each account held, for the transfers proposed from it.
    pub held: BTreeMap<AccountId, Chain>,
    /// The ledgers of the ICRC accounts, which become tracked.
    pub ledgers: Vec<Chain>,
}

/// Gives the accounts stored before kinds their kind. ICP and ICRC accounts
/// that share an index were one subaccount, so they become one Internet
/// Computer account: the oldest keeps its id and label, and the others lead
/// to it. A Bitcoin account keeps its network, index and key.
pub fn migrate(accounts: &[(AccountId, StoredAccount)]) -> Result<Migration, String> {
    let mut migration = Migration::default();
    // The Internet Computer account each index became, by the oldest id.
    let mut survivors: BTreeMap<u32, AccountId> = BTreeMap::new();
    for (id, account) in accounts {
        let (Some(chain), None) = (&account.chain, account.kind) else {
            continue;
        };
        let kind = match chain {
            Chain::Icp | Chain::Icrc(_) => AccountKind::InternetComputer,
            Chain::Bitcoin(network) => AccountKind::Bitcoin(*network),
            Chain::Evm(_) => return Err(format!("account {id} holds an EVM chain")),
        };
        if let Chain::Icrc(ledger) = chain
            && *ledger != ICP_LEDGER
            && !migration.ledgers.contains(chain)
        {
            migration.ledgers.push(chain.clone());
        }
        migration.held.insert(*id, chain.clone());
        let mut migrated = StoredAccount {
            kind: Some(kind),
            chain: None,
            ..account.clone()
        };
        if kind == AccountKind::InternetComputer && account.holder == AccountHolder::Wallet {
            let survivor = *survivors.entry(account.index).or_insert(*id);
            if survivor != *id {
                migrated.merged_into = Some(survivor);
            }
        }
        migration.accounts.push((*id, migrated));
    }
    Ok(migration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn wallet() -> Principal {
        Principal::from_text("uxrrr-q7777-77774-qaaaq-cai").unwrap()
    }

    fn ledger_x() -> Principal {
        Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap()
    }

    const IC: AccountKind = AccountKind::InternetComputer;
    const REGTEST: AccountKind = AccountKind::Bitcoin(BitcoinNetwork::Regtest);
    const EVM: AccountKind = AccountKind::Evm;

    fn stored(kind: AccountKind, index: u32) -> StoredAccount {
        StoredAccount::new(
            kind,
            format!("{kind:?} {index}"),
            index,
            None,
            Some(Principal::self_authenticating([1; 32])),
            0,
        )
    }

    /// An account as a release before kinds stored it.
    fn legacy(chain: Chain, index: u32) -> StoredAccount {
        StoredAccount {
            kind: None,
            chain: Some(chain),
            ..stored(IC, index)
        }
    }

    fn args(kind: AccountKind) -> AddAccountArgs {
        AddAccountArgs {
            kind,
            label: "savings".to_string(),
        }
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

        assert_eq!(stored(IC, 0).view(7, wallet()).address, wallet().to_text());
        let address = stored(IC, 1).view(7, wallet()).address;
        assert!(address.starts_with(&format!("{}-", wallet())), "{address}");
        assert!(address.ends_with(".1"), "{address}");
    }

    #[test]
    fn an_address_reads_back_as_its_ledger_account() {
        for index in [0, 1, 255, 256, u32::MAX] {
            let account = stored(IC, index);
            let address = account.view(1, wallet()).address;
            assert_eq!(
                LedgerAccount::from_str(&address),
                Ok(account.ledger_account(wallet())),
                "{address}"
            );
        }
    }

    #[test]
    fn crc32_is_zlibs() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn an_internet_computer_account_shows_its_icp_account_identifier() {
        // The ICP ledger's identifier of the anonymous principal's default
        // account.
        let anonymous = LedgerAccount::from(Principal::anonymous());
        assert_eq!(
            hex(&account_identifier(&anonymous)),
            "1c7a48ba6a562aa9eaa2481a9049cdf0433b9738c992d698c31d8abf89cadc79"
        );
        let view = stored(IC, 0).view(1, wallet());
        let id = view.icp_account_id.unwrap();
        assert_eq!(id.len(), 64);
        assert_ne!(Some(id), stored(IC, 1).view(1, wallet()).icp_account_id);
        assert_eq!(stored(REGTEST, 0).view(1, wallet()).icp_account_id, None);
    }

    #[test]
    fn indices_count_per_kind_from_zero_and_never_repeat() {
        let mut merged = stored(IC, 2);
        merged.merged_into = Some(1);
        let existing = [stored(IC, 0), stored(IC, 1), stored(REGTEST, 0), merged];
        assert_eq!(check_new(&args(IC), &existing), Ok(3));
        assert_eq!(check_new(&args(REGTEST), &existing), Ok(1));
        assert_eq!(
            check_new(
                &args(AccountKind::Bitcoin(BitcoinNetwork::Mainnet)),
                &existing
            ),
            Ok(0)
        );
        assert_eq!(check_new(&args(IC), &[]), Ok(0));
    }

    #[test]
    fn a_bitcoin_account_shows_the_address_of_its_key() {
        use crate::btc::tests::{BIP173_KEY, BIP173_REGTEST, hex};
        let account = StoredAccount {
            public_key: Some(ByteBuf::from(hex(BIP173_KEY))),
            ..stored(REGTEST, 0)
        };
        assert_eq!(account.view(1, wallet()).address, BIP173_REGTEST);
        assert!(stored(IC, 0).bitcoin_address().is_err());
    }

    #[test]
    fn an_evm_account_shows_the_checksummed_address_of_its_key() {
        let account = StoredAccount {
            public_key: Some(ByteBuf::from(crate::evm::tests::public_key())),
            ..stored(EVM, 0)
        };
        let view = account.view(1, wallet());
        assert_eq!(view.address, "0x9d8A62f656a8d1615C1294fd71e9CFb3E4855A4F");
        assert_eq!(view.icp_account_id, None);
        assert!(stored(REGTEST, 0).evm_address().is_err());
        assert!(account.bitcoin_address().is_err());
        assert_eq!(check_new(&args(EVM), &[account]), Ok(1));
    }

    #[test]
    fn refuses_bad_labels() {
        let blank = AddAccountArgs {
            kind: IC,
            label: " ".to_string(),
        };
        assert!(matches!(
            check_new(&blank, &[]),
            Err(WalletError::InvalidArgument(_))
        ));
    }

    #[test]
    fn names_are_unique_as_the_names_rule_compares_them() {
        assert_eq!(name_key("  BTC\u{a0}  Savings "), "btc savings");
        assert_eq!(name_key("\u{ff33}avings"), "savings"); // a fullwidth S is an S (NFKC)
        let mut savings = stored(IC, 1);
        savings.label = "BTC Savings".to_string();
        let existing = [stored(IC, 0), savings.clone()];
        for taken in ["btc savings", " BTC  SAVINGS", "\u{ff22}TC Savings"] {
            let new = AddAccountArgs {
                kind: EVM,
                label: taken.to_string(),
            };
            assert!(
                matches!(
                    check_new(&new, &existing),
                    Err(WalletError::InvalidArgument(_))
                ),
                "{taken}"
            );
        }
        let other = AddAccountArgs {
            kind: EVM,
            label: "BTC Savings 2".into(),
        };
        assert_eq!(check_new(&other, &existing), Ok(0));
        // A merged account is not listed, so its name is free.
        let mut merged = savings.clone();
        merged.merged_into = Some(1);
        let same = AddAccountArgs {
            kind: EVM,
            label: "BTC Savings".into(),
        };
        assert_eq!(check_new(&same, &[merged]), Ok(0));
        // Renaming an account to its own name, in another case, is no clash.
        let ids = [(1, &existing[0]), (2, &existing[1])];
        assert_eq!(check_unique("btc SAVINGS", Some(2), ids), Ok(()));
        assert!(check_unique("btc SAVINGS", Some(1), ids).is_err());
    }

    #[test]
    fn the_wallets_own_accounts_get_names_of_their_own_and_old_defaults_are_renamed() {
        assert_eq!(
            [IC, REGTEST, EVM].map(default_label),
            ["ICP wallet", "Bitcoin wallet", "Ethereum wallet"]
        );
        let made =
            |kind, index| StoredAccount::new(kind, OLD_DEFAULT_LABEL.into(), index, None, None, 0);
        let mut taken = stored(IC, 3);
        taken.label = "bitcoin WALLET".into(); // an owner took the default name already
        let mut kept = stored(EVM, 1);
        kept.label = OLD_DEFAULT_LABEL.into(); // an owner named it "Main": it keeps the name
        // A wallet-made account an owner renamed "Main" keeps it on every later upgrade.
        let mut chosen = made(IC, 1);
        chosen.renamed_by = Some(Principal::anonymous());
        let all = [
            (1, made(IC, 0)),
            (2, made(REGTEST, 0)),
            (3, made(EVM, 0)),
            (4, taken),
            (5, kept),
            (6, chosen),
        ];
        let renamed: Vec<(AccountId, String)> = rename_defaults(&all)
            .into_iter()
            .map(|(id, a)| (id, a.label))
            .collect();
        let want: Vec<(AccountId, String)> = vec![
            (1, "ICP wallet".into()),
            (2, "Bitcoin wallet 2".into()),
            (3, "Ethereum wallet".into()),
        ];
        assert_eq!(renamed, want);
        assert_eq!(free_label("ICP wallet", [&made(IC, 0)]), "ICP wallet");
    }

    #[test]
    fn a_wallet_has_at_most_sixty_four_listed_accounts() {
        let full: Vec<StoredAccount> = (0..MAX_ACCOUNTS as u32)
            .map(|index| stored(IC, index))
            .collect();
        assert!(matches!(
            check_new(&args(IC), &full),
            Err(WalletError::PolicyViolation(_))
        ));
        let mut one_merged = full.clone();
        one_merged[5].merged_into = Some(1);
        assert_eq!(check_new(&args(IC), &one_merged), Ok(64));
    }

    #[test]
    fn a_wallet_lacks_the_defaults_it_has_no_account_zero_of() {
        let all = [IC, REGTEST, EVM];
        assert_eq!(missing_defaults(BitcoinNetwork::Regtest, &[]), all);
        let existing = [stored(IC, 1), stored(REGTEST, 0), stored(EVM, 0)];
        assert_eq!(missing_defaults(BitcoinNetwork::Regtest, &existing), [IC]);
        assert_eq!(
            missing_defaults(BitcoinNetwork::Mainnet, &existing),
            [IC, AccountKind::Bitcoin(BitcoinNetwork::Mainnet)]
        );
        let mut merged = stored(IC, 0);
        merged.merged_into = Some(9);
        assert_eq!(
            missing_defaults(BitcoinNetwork::Regtest, &[merged, stored(REGTEST, 0)]),
            [IC, EVM]
        );
        assert!(
            missing_defaults(
                BitcoinNetwork::Regtest,
                &[stored(IC, 0), stored(REGTEST, 0), stored(EVM, 0)]
            )
            .is_empty()
        );
    }

    #[test]
    fn the_migration_merges_icp_and_icrc_accounts_of_an_index_into_the_oldest() {
        let regtest = Chain::Bitcoin(BitcoinNetwork::Regtest);
        let accounts = vec![
            (1, legacy(Chain::Icp, 0)),
            (2, legacy(Chain::Icrc(ledger_x()), 0)),
            (3, legacy(Chain::Icp, 1)),
            (4, legacy(regtest.clone(), 0)),
            (5, legacy(Chain::Icrc(ledger_x()), 1)),
            (6, stored(IC, 2)),
        ];
        let migration = migrate(&accounts).unwrap();
        let kinds: Vec<(AccountId, AccountKind, Option<AccountId>)> = migration
            .accounts
            .iter()
            .map(|(id, account)| (*id, account.kind(), account.merged_into))
            .collect();
        assert_eq!(
            kinds,
            [
                (1, IC, None),
                (2, IC, Some(1)),
                (3, IC, None),
                (4, REGTEST, None),
                (5, IC, Some(3)),
            ]
        );
        assert!(migration.accounts.iter().all(|(_, a)| a.chain.is_none()));
        assert_eq!(migration.ledgers, [Chain::Icrc(ledger_x())]);
        assert_eq!(migration.held[&2], Chain::Icrc(ledger_x()));
        assert_eq!(migration.held[&4], regtest);
        assert!(!migration.held.contains_key(&6));
        // Run again, it changes nothing.
        let again: Vec<(AccountId, StoredAccount)> = migration.accounts.clone();
        assert_eq!(migrate(&again), Ok(Migration::default()));
        assert!(migrate(&[(1, legacy(Chain::Evm(1), 0))]).is_err());
    }
}
