//! Stable memory. Every store lives in its own virtual memory of one
//! `MemoryManager`, so state is written in place and an upgrade never
//! serializes the heap. The memory ids are listed in `docs/design.md`; an id
//! is never reused or renumbered, even after its store goes away.
//!
//! Stored values are Candid. A release may add a field to a stored record only
//! as an `opt` field, so the values an earlier release wrote still decode.

use crate::accounts::{self, StoredAccount};
use crate::actions::StoredPending;
use crate::assets;
use crate::btc;
use crate::governance::Governance;
use crate::policy::default_policy;
use crate::upgrade::StoredUpgrade;
use crate::{
    AccountId, AccountKind, Action, ActionId, ActionState, AuditEntry, AuditEvent, BitcoinNetwork,
    Chain, InitArgs, Member, Outcome, Policy, Role,
};
use candid::{CandidType, Nat, Principal, decode_one, encode_one};
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::storable::Bound;
use ic_stable_structures::{DefaultMemoryImpl, StableBTreeMap, StableCell, StableLog, Storable};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_bytes::ByteBuf;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

type Memory = VirtualMemory<DefaultMemoryImpl>;

const CONFIG_MEMORY: MemoryId = MemoryId::new(0);
const POLICY_MEMORY: MemoryId = MemoryId::new(1);
const MEMBERS_MEMORY: MemoryId = MemoryId::new(2);
const ACCOUNTS_MEMORY: MemoryId = MemoryId::new(3);
const PENDING_MEMORY: MemoryId = MemoryId::new(4);
const HISTORY_MEMORY: MemoryId = MemoryId::new(6);
const AUDIT_INDEX_MEMORY: MemoryId = MemoryId::new(7);
const AUDIT_DATA_MEMORY: MemoryId = MemoryId::new(8);
const UPGRADE_MEMORY: MemoryId = MemoryId::new(9);
const OPEN_MEMORY: MemoryId = MemoryId::new(10);
const TRANSFER_CLOCK_MEMORY: MemoryId = MemoryId::new(11);
const RESERVED_MEMORY: MemoryId = MemoryId::new(12);
const TRACKED_MEMORY: MemoryId = MemoryId::new(13);
const UNSETTLED_MEMORY: MemoryId = MemoryId::new(14);

/// What the wallet was installed with.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Present until the wallet is claimed.
    pub claim: Option<PendingClaim>,
    pub release_channel: Principal,
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
    pub factory: Option<Principal>,
    /// The network of the default Bitcoin account and its keys. Absent only
    /// from wallets installed before it was set; `migrate` sets it.
    pub bitcoin_network: Option<BitcoinNetwork>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PendingClaim {
    pub public_key: ByteBuf,
    pub expires_at_ns: u64,
}

/// A member, stored under its principal.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredMember {
    pub role: Role,
    pub label: String,
    pub added_at_ns: u64,
    /// `None` for the member who claimed the wallet.
    pub added_by: Option<Principal>,
}

/// The assets the accounts show, ICP first.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Tracked {
    pub assets: Vec<Chain>,
}

/// An audit log entry as stored; its sequence number is its place in the log.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredAuditEntry {
    pub at_ns: u64,
    pub by: Option<Principal>,
    pub event: AuditEvent,
}

/// A transfer the wallet sent, stored under its sequence number.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredTransfer {
    pub account: AccountId,
    pub chain: Chain,
    pub to: String,
    pub amount: Nat,
    /// The ledger block that holds it, in decimal, or the Bitcoin
    /// transaction id: the receipt's `tx_id`.
    pub tx_id: String,
    pub at_ns: u64,
    pub action: ActionId,
    /// The fee a Bitcoin transaction paid, which counts toward the daily
    /// limit. Absent for ledger transfers, whose fee does not.
    pub fee: Option<Nat>,
}

impl StoredMember {
    /// The member as the interface shows it.
    pub fn into_member(self, id: Principal) -> Member {
        Member {
            id,
            role: self.role,
            label: self.label,
            added_at_ns: self.added_at_ns,
            added_by: self.added_by,
        }
    }
}

impl From<InitArgs> for Config {
    fn from(args: InitArgs) -> Self {
        Config {
            claim: Some(PendingClaim {
                public_key: args.claim_public_key,
                expires_at_ns: args.claim_expires_at_ns,
            }),
            release_channel: args.release_channel,
            release_signers: args.release_signers,
            release_threshold: args.release_threshold,
            factory: args.factory,
            bitcoin_network: Some(args.bitcoin_network.unwrap_or(BitcoinNetwork::Mainnet)),
        }
    }
}

macro_rules! candid_storable {
    ($($stored:ty),*) => {$(
        impl Storable for $stored {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(to_candid(self))
            }

            fn into_bytes(self) -> Vec<u8> {
                to_candid(&self)
            }

            fn from_bytes(bytes: Cow<[u8]>) -> Self {
                from_candid(&bytes)
            }

            const BOUND: Bound = Bound::Unbounded;
        }
    )*};
}

candid_storable!(
    Config,
    Policy,
    StoredAccount,
    StoredAuditEntry,
    StoredMember,
    StoredPending,
    StoredTransfer,
    StoredUpgrade,
    Tracked
);

fn to_candid<T: CandidType>(value: &T) -> Vec<u8> {
    encode_one(value).expect("a stored value encodes as Candid")
}

// A value that does not decode traps the call, which rolls back its changes.
fn from_candid<T: CandidType + DeserializeOwned>(bytes: &[u8]) -> T {
    decode_one(bytes).unwrap_or_else(|e| panic!("a stored value does not decode: {e}"))
}

thread_local! {
    static MEMORY_MANAGER: MemoryManager<DefaultMemoryImpl> =
        MemoryManager::init(DefaultMemoryImpl::default());

    static CONFIG: RefCell<StableCell<Option<Config>, Memory>> =
        RefCell::new(StableCell::init(memory(CONFIG_MEMORY), None));

    static POLICY: RefCell<StableCell<Option<Policy>, Memory>> =
        RefCell::new(StableCell::init(memory(POLICY_MEMORY), None));

    static MEMBERS: RefCell<StableBTreeMap<Principal, StoredMember, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(MEMBERS_MEMORY)));

    static ACCOUNTS: RefCell<StableBTreeMap<AccountId, StoredAccount, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(ACCOUNTS_MEMORY)));

    static PENDING: RefCell<StableBTreeMap<ActionId, StoredPending, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(PENDING_MEMORY)));

    /// The transfers the wallet sent, oldest first.
    static HISTORY: RefCell<StableBTreeMap<u64, StoredTransfer, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(HISTORY_MEMORY)));

    /// The ids of the actions not yet executed, rejected, expired or failed,
    /// so finding them never reads the whole history.
    static OPEN: RefCell<StableBTreeMap<ActionId, (), Memory>> =
        RefCell::new(StableBTreeMap::init(memory(OPEN_MEMORY)));

    /// The self-upgrade: staging, installing, or how the last one ended.
    /// Absent until the first upgrade a wallet runs itself.
    static UPGRADE: RefCell<StableCell<Option<StoredUpgrade>, Memory>> =
        RefCell::new(StableCell::init(memory(UPGRADE_MEMORY), None));

    static AUDIT: RefCell<StableLog<StoredAuditEntry, Memory, Memory>> =
        RefCell::new(StableLog::init(memory(AUDIT_INDEX_MEMORY), memory(AUDIT_DATA_MEMORY)));

    /// The latest `created_at_time` the wallet gave a ledger call.
    static TRANSFER_CLOCK: RefCell<StableCell<u64, Memory>> =
        RefCell::new(StableCell::init(memory(TRANSFER_CLOCK_MEMORY), 0));

    /// The Bitcoin outputs a built transaction spends, as `btc::Coin::key`
    /// writes them, to the action whose transaction it is. No other
    /// transaction spends them. They stay reserved once the transaction may
    /// have been sent: the Bitcoin canister lists them until it confirms.
    static RESERVED: RefCell<StableBTreeMap<[u8; 36], ActionId, Memory>> =
        RefCell::new(StableBTreeMap::init(memory(RESERVED_MEMORY)));

    static TRACKED: RefCell<StableCell<Option<Tracked>, Memory>> =
        RefCell::new(StableCell::init(memory(TRACKED_MEMORY), None));

    /// The actions the wallet still settles on its own: see
    /// `StoredPending::needs_settling`.
    static UNSETTLED: RefCell<StableBTreeMap<ActionId, (), Memory>> =
        RefCell::new(StableBTreeMap::init(memory(UNSETTLED_MEMORY)));
}

fn memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|manager| manager.get(id))
}

/// Stores the configuration at install.
pub fn set_config(config: Config) {
    CONFIG.with_borrow_mut(|cell| cell.set(Some(config)));
}

/// Stores the policy at install; afterwards only `save_governance` changes it.
pub fn set_policy(policy: Policy) {
    POLICY.with_borrow_mut(|cell| cell.set(Some(policy)));
}

pub fn policy() -> Policy {
    POLICY
        .with_borrow(|cell| cell.get().clone())
        .expect("an installed wallet has a policy")
}

pub fn config() -> Option<Config> {
    CONFIG.with_borrow(|cell| cell.get().clone())
}

/// The network of the wallet's default Bitcoin account.
pub fn bitcoin_network() -> BitcoinNetwork {
    config()
        .and_then(|config| config.bitcoin_network)
        .unwrap_or(BitcoinNetwork::Mainnet)
}

/// The wallet's Bitcoin network once it has a Bitcoin account, which an
/// upgrade can no longer change. Until then an upgrade may set it, so a
/// wallet that took the wrong one gets its default Bitcoin account on the
/// right one. Reads accounts stored before kinds too.
pub fn fixed_bitcoin_network() -> Option<BitcoinNetwork> {
    let first =
        accounts()
            .into_iter()
            .find_map(|(_, account)| match (account.kind, account.chain) {
                (Some(AccountKind::Bitcoin(network)), _)
                | (None, Some(Chain::Bitcoin(network))) => Some(network),
                _ => None,
            })?;
    Some(
        config()
            .and_then(|config| config.bitcoin_network)
            .unwrap_or(first),
    )
}

/// The tracked assets, ICP first.
pub fn tracked() -> Vec<Chain> {
    TRACKED
        .with_borrow(|cell| cell.get().clone().map(|tracked| tracked.assets))
        .unwrap_or_else(assets::initial)
}

pub fn set_tracked(assets: Vec<Chain>) {
    TRACKED.with_borrow_mut(|cell| cell.set(Some(Tracked { assets })));
}

pub fn upgrade() -> StoredUpgrade {
    UPGRADE.with_borrow(|cell| cell.get().clone().unwrap_or_default())
}

pub fn set_upgrade(upgrade: StoredUpgrade) {
    UPGRADE.with_borrow_mut(|cell| cell.set(Some(upgrade)));
}

pub fn is_claimed() -> bool {
    MEMBERS.with_borrow(|members| !members.is_empty())
}

pub fn member(id: Principal) -> Option<StoredMember> {
    MEMBERS.with_borrow(|members| members.get(&id))
}

/// Every member, in principal order.
pub fn members() -> Vec<Member> {
    MEMBERS.with_borrow(|members| {
        members
            .iter()
            .map(|entry| {
                let (id, member) = entry.into_pair();
                member.into_member(id)
            })
            .collect()
    })
}

#[cfg(test)]
pub fn insert_member(id: Principal, member: StoredMember) {
    MEMBERS.with_borrow_mut(|members| members.insert(id, member));
}

/// The members and the policy, read into the heap. A wallet has at most
/// `MAX_MEMBERS` members, so this stays small.
pub fn governance() -> Governance {
    Governance {
        members: MEMBERS
            .with_borrow(|members| members.iter().map(|entry| entry.into_pair()).collect()),
        policy: policy(),
    }
}

/// Writes back what an applied action changed.
pub fn save_governance(governance: &Governance) {
    MEMBERS.with_borrow_mut(|members| {
        let gone: Vec<Principal> = members
            .keys()
            .filter(|id| !governance.members.contains_key(id))
            .collect();
        for id in gone {
            members.remove(&id);
        }
        for (id, member) in &governance.members {
            if members.get(id).as_ref() != Some(member) {
                members.insert(*id, member.clone());
            }
        }
    });
    if policy() != governance.policy {
        set_policy(governance.policy.clone());
    }
}

/// Stores a new account under the next id. Ids start at 1 and only grow, and
/// accounts are never deleted.
pub fn add_account(account: StoredAccount) -> AccountId {
    ACCOUNTS.with_borrow_mut(|accounts| {
        let id = accounts.last_key_value().map_or(1, |(last, _)| last + 1);
        accounts.insert(id, account);
        id
    })
}

pub fn account(id: AccountId) -> Option<StoredAccount> {
    ACCOUNTS.with_borrow(|accounts| accounts.get(&id))
}

/// The listed account `id` names: itself, or for an account merged into
/// another, the one it became.
pub fn listed_account(id: AccountId) -> Option<(AccountId, StoredAccount)> {
    let account = account(id)?;
    match account.merged_into {
        Some(survivor) => self::account(survivor).map(|account| (survivor, account)),
        None => Some((id, account)),
    }
}

/// Every account, merged ones included, in id order. A wallet lists at most
/// `MAX_ACCOUNTS`, and merged ones are only those an earlier release had.
pub fn accounts() -> Vec<(AccountId, StoredAccount)> {
    ACCOUNTS.with_borrow(|accounts| accounts.iter().map(|entry| entry.into_pair()).collect())
}

/// Records a transfer the ledger holds.
pub fn add_transfer(transfer: StoredTransfer) {
    HISTORY.with_borrow_mut(|history| {
        let seq = history.last_key_value().map_or(0, |(last, _)| last + 1);
        history.insert(seq, transfer);
    });
}

/// A `created_at_time` for a new ledger call: `now`, or a nanosecond after
/// the latest one when that is later. Messages of one round share their time,
/// so without this two transfers with the same arguments executed together
/// would look like one to the ledger.
pub fn transfer_time(now: u64) -> u64 {
    TRANSFER_CLOCK.with_borrow_mut(|clock| {
        let time = now.max(clock.get().saturating_add(1));
        clock.set(time);
        time
    })
}

/// What the wallet sent on `chain` at `since_ns` or later, with the fees
/// of Bitcoin transactions. It reads back from the newest transfer and stops
/// at the first older one.
pub fn sent_since(chain: &Chain, since_ns: u64) -> Nat {
    HISTORY.with_borrow(|history| {
        history
            .values()
            .rev()
            .take_while(|transfer| transfer.at_ns >= since_ns)
            .filter(|transfer| &transfer.chain == chain)
            .fold(Nat::from(0u8), |sum, transfer| {
                sum + transfer.amount + transfer.fee.unwrap_or_default()
            })
    })
}

/// Reserves the outputs of `keys` for the transaction of action `id`.
pub fn reserve(keys: &[[u8; 36]], id: ActionId) {
    RESERVED.with_borrow_mut(|reserved| {
        for key in keys {
            reserved.insert(*key, id);
        }
    });
}

/// Whether a built transaction spends the output of `key`.
pub fn is_reserved(key: &[u8; 36]) -> bool {
    RESERVED.with_borrow(|reserved| reserved.contains_key(key))
}

/// Releases the outputs of `keys` that action `id` reserved.
fn release(keys: &[[u8; 36]], id: ActionId) {
    RESERVED.with_borrow_mut(|reserved| {
        for key in keys {
            if reserved.get(key) == Some(id) {
                reserved.remove(key);
            }
        }
    });
}

/// Stores a new action under the next id. Ids start at 1 and only grow, and
/// actions are never deleted, so an id never names another action.
pub fn add_action(pending: StoredPending) -> ActionId {
    let id = PENDING.with_borrow_mut(|actions| {
        let id = actions.last_key_value().map_or(1, |(last, _)| last + 1);
        actions.insert(id, pending);
        id
    });
    OPEN.with_borrow_mut(|open| open.insert(id, ()));
    id
}

pub fn action(id: ActionId) -> Option<StoredPending> {
    PENDING.with_borrow(|actions| actions.get(&id))
}

/// Stores an action. One that closed without its Bitcoin transaction ever
/// being sent releases the outputs that transaction would have spent, as
/// does one whose transaction settled. An action the wallet still settles
/// on its own is listed for the timer, and one it no longer does is not.
pub fn set_action(id: ActionId, pending: StoredPending) {
    let closed = !matches!(pending.state, ActionState::Waiting | ActionState::Ready);
    if let Some(tx) = pending.bitcoin_tx()
        && ((closed && !pending.may_have_sent()) || tx.is_settled())
    {
        // A stored transaction always decodes: the wallet encoded it.
        let unsigned = btc::from_bytes(&tx.unsigned).expect("a stored transaction decodes");
        release(&btc::spent_keys(&unsigned), id);
    }
    let settling = pending.needs_settling();
    PENDING.with_borrow_mut(|actions| actions.insert(id, pending));
    if closed {
        OPEN.with_borrow_mut(|open| open.remove(&id));
    }
    UNSETTLED.with_borrow_mut(|unsettled| {
        if settling {
            unsettled.insert(id, ());
        } else {
            unsettled.remove(&id);
        }
    });
}

/// The ids of the actions the wallet still settles on its own, oldest
/// first.
pub fn unsettled() -> Vec<ActionId> {
    UNSETTLED.with_borrow(|unsettled| unsettled.keys().collect())
}

/// The actions open at `now`, oldest first.
pub fn open_actions(now: u64) -> Vec<(ActionId, StoredPending)> {
    let ids: Vec<ActionId> = OPEN.with_borrow(|open| open.keys().collect());
    ids.into_iter()
        .map(|id| (id, action(id).expect("an open id names a stored action")))
        .filter(|(_, pending)| pending.is_open(now))
        .collect()
}

/// Writes down the expiry of every action that expired by `now`, which also
/// takes it out of the open ids. Run before adding an action, it keeps the
/// open ids to the open actions and one more.
pub fn close_expired(now: u64) {
    let ids: Vec<ActionId> = OPEN.with_borrow(|open| open.keys().collect());
    for id in ids {
        let mut pending = action(id).expect("an open id names a stored action");
        if !pending.is_open(now) {
            pending.expire(now);
            set_action(id, pending);
        }
    }
}

/// Appends to the audit log. A wallet out of stable memory traps here, which
/// rolls back the change the entry records: nothing happens unlogged.
pub fn append_audit(entry: StoredAuditEntry) -> u64 {
    AUDIT.with_borrow(|log| log.append(&entry).expect("the audit log grows"))
}

/// Up to `limit` entries before `before_seq` (or the end), newest first.
pub fn audit_page(before_seq: Option<u64>, limit: usize) -> Vec<AuditEntry> {
    AUDIT.with_borrow(|log| {
        let end = before_seq.map_or(log.len(), |before| before.min(log.len()));
        (0..end)
            .rev()
            .take(limit)
            .map(|seq| {
                let entry = log.get(seq).expect("an entry below the length is stored");
                AuditEntry {
                    seq,
                    at_ns: entry.at_ns,
                    by: entry.by,
                    event: entry.event,
                }
            })
            .collect()
    })
}

/// Adds the first owner and deletes the claim key, in the same message, so
/// the claim cannot be used twice.
pub fn complete_claim(owner: Principal, member: StoredMember) {
    CONFIG.with_borrow_mut(|cell| {
        let mut config = cell
            .get()
            .clone()
            .expect("a claimed wallet has a configuration");
        config.claim = None;
        cell.set(Some(config));
    });
    MEMBERS.with_borrow_mut(|members| members.insert(owner, member));
}

/// Fills in what an earlier release did not store, so its wallets can
/// upgrade. A trap later in the upgrade rolls this back with the rest.
///
/// - Wallets from before step 5 have no policy and get the default one,
///   which is the policy a new wallet starts with.
/// - Accounts stored before kinds get theirs (see `accounts::migrate`), and
///   the transfers proposed from them the asset they send and the account
///   they now name. The ICRC-1 ledgers of the old accounts become tracked.
/// - A wallet with a Bitcoin account keeps its Bitcoin network, or takes
///   that of its first Bitcoin account; one without takes `network`, else
///   keeps its own, else mainnet.
/// - Transfers that may have run unseen, and Bitcoin transactions that may
///   have been sent, are settled from now on.
pub fn migrate(network: Option<BitcoinNetwork>) -> Result<(), String> {
    if POLICY.with_borrow(|cell| cell.get().is_none()) {
        set_policy(default_policy());
    }
    let migration = accounts::migrate(&accounts())?;
    ACCOUNTS.with_borrow_mut(|stored| {
        for (id, account) in &migration.accounts {
            stored.insert(*id, account.clone());
        }
    });
    let mut config = config().ok_or("stable memory holds no configuration")?;
    // `post_upgrade` refuses a `network` other than a fixed one.
    let chosen = fixed_bitcoin_network()
        .or(network)
        .or(config.bitcoin_network)
        .unwrap_or(BitcoinNetwork::Mainnet);
    if config.bitcoin_network != Some(chosen) {
        config.bitcoin_network = Some(chosen);
        set_config(config);
    }
    if TRACKED.with_borrow(|cell| cell.get().is_none()) {
        let mut tracked = assets::initial();
        tracked.extend(migration.ledgers.iter().cloned());
        set_tracked(tracked);
    }
    if !migration.held.is_empty() {
        migrate_transfers(&migration.held);
    }
    // A transfer that needs settling is open, or its transaction reserves
    // outputs.
    let mut candidates: BTreeSet<ActionId> = OPEN.with_borrow(|open| open.keys().collect());
    candidates.extend(RESERVED.with_borrow(|reserved| reserved.values().collect::<Vec<_>>()));
    for id in candidates {
        if let Some(pending) = action(id)
            && pending.needs_settling()
        {
            UNSETTLED.with_borrow_mut(|unsettled| unsettled.insert(id, ()));
        }
    }
    Ok(())
}

/// Names in every transfer what it sends, by what its account held, and
/// the account it was sent from as the one that account became.
fn migrate_transfers(held: &BTreeMap<AccountId, Chain>) {
    let survivor = |id: AccountId| {
        account(id)
            .and_then(|account| account.merged_into)
            .unwrap_or(id)
    };
    let ids: Vec<ActionId> = PENDING.with_borrow(|actions| actions.keys().collect());
    for id in ids {
        let mut pending = action(id).expect("a listed id names a stored action");
        let Action::Transfer(request) = &mut pending.action else {
            continue;
        };
        if request.asset.is_some() {
            continue;
        }
        request.asset = held.get(&request.from).cloned();
        request.from = survivor(request.from);
        if let ActionState::Executed(Outcome::Sent(receipt)) = &mut pending.state {
            receipt.account = survivor(receipt.account);
        }
        PENDING.with_borrow_mut(|actions| actions.insert(id, pending));
    }
    let seqs: Vec<u64> = HISTORY.with_borrow(|history| history.keys().collect());
    for seq in seqs {
        HISTORY.with_borrow_mut(|history| {
            let mut transfer = history.get(&seq).expect("a listed transfer is stored");
            let account = survivor(transfer.account);
            if account != transfer.account {
                transfer.account = account;
                history.insert(seq, transfer);
            }
        });
    }
}

/// Decodes every stored value and checks what holds them together.
/// `post_upgrade` calls it, so a release that cannot read the state fails
/// its upgrade instead of a later call.
pub fn check() -> Result<(), String> {
    let Some(config) = config() else {
        return Err("stable memory holds no configuration".to_string());
    };
    if config.bitcoin_network.is_none() {
        return Err("the configuration has no Bitcoin network".to_string());
    }
    if POLICY.with_borrow(|cell| cell.get().is_none()) {
        return Err("stable memory holds no policy".to_string());
    }
    MEMBERS.with_borrow(|members| members.values().for_each(drop));
    PENDING.with_borrow(|actions| actions.values().for_each(drop));
    HISTORY.with_borrow(|history| history.values().for_each(drop));
    AUDIT.with_borrow(|log| log.iter().for_each(drop));
    for (id, account) in accounts() {
        let Some(kind) = account.kind else {
            return Err(format!("account {id} has no kind"));
        };
        if let Some(survivor) = account.merged_into
            && !listed_account(survivor).is_some_and(|(at, into)| {
                at == survivor && into.kind == Some(kind) && into.index == account.index
            })
        {
            return Err(format!(
                "account {id} leads to account {survivor}, which is not its listed twin"
            ));
        }
    }
    let tracked = TRACKED
        .with_borrow(|cell| cell.get().clone())
        .ok_or("stable memory holds no tracked assets")?
        .assets;
    if tracked.first() != Some(&Chain::Icp) {
        return Err("ICP is not the first tracked asset".to_string());
    }
    for asset in &tracked[1..] {
        if !matches!(asset, Chain::Icrc(_)) {
            return Err(format!("{asset:?} is tracked"));
        }
        assets::check(asset)?;
    }
    let upgrade = upgrade();
    if let Some(id) = upgrade.action
        && action(id).is_none()
    {
        return Err(format!(
            "the upgrade belongs to action {id}, which is not stored"
        ));
    }
    if let Some(id) =
        RESERVED.with_borrow(|reserved| reserved.values().find(|id| action(*id).is_none()))
    {
        return Err(format!(
            "Bitcoin outputs are reserved for action {id}, which is not stored"
        ));
    }
    if let Some(id) = OPEN.with_borrow(|open| open.keys().find(|id| action(*id).is_none())) {
        return Err(format!("open action {id} is not stored"));
    }
    if let Some(id) =
        UNSETTLED.with_borrow(|unsettled| unsettled.keys().find(|id| action(*id).is_none()))
    {
        return Err(format!("unsettled action {id} is not stored"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use crate::actions::APPROVAL_WINDOW_NS;
    use crate::policy::Terms;

    fn config_value() -> Config {
        Config {
            claim: Some(PendingClaim {
                public_key: ByteBuf::from(vec![1; 32]),
                expires_at_ns: 7,
            }),
            release_channel: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
            release_signers: vec![ByteBuf::from(vec![2; 32])],
            release_threshold: 1,
            factory: None,
            bitcoin_network: Some(crate::BitcoinNetwork::Regtest),
        }
    }

    fn stored_member() -> StoredMember {
        StoredMember {
            role: Role::Owner,
            label: "laptop".to_string(),
            added_at_ns: 9,
            added_by: None,
        }
    }

    #[test]
    fn stored_values_round_trip_through_their_bytes() {
        let config = config_value();
        assert_eq!(Config::from_bytes(config.to_bytes()), config);
        let member = stored_member();
        assert_eq!(StoredMember::from_bytes(member.to_bytes()), member);
    }

    /// The rule every release follows: a field added later is `opt`, so the
    /// bytes written now still decode.
    #[test]
    fn a_later_release_can_add_optional_fields() {
        #[derive(CandidType, Deserialize, Debug)]
        struct LaterMember {
            role: Role,
            label: String,
            added_at_ns: u64,
            added_by: Option<Principal>,
            device_name: Option<String>,
        }

        let later: LaterMember = from_candid(&stored_member().to_bytes());
        assert_eq!(later.role, Role::Owner);
        assert_eq!(later.label, "laptop");
        assert_eq!(later.added_at_ns, 9);
        assert_eq!(later.added_by, None);
        assert_eq!(later.device_name, None);
    }

    #[test]
    #[should_panic(expected = "does not decode")]
    fn a_required_field_cannot_be_added() {
        #[derive(CandidType, Deserialize, Debug)]
        struct BrokenMember {
            #[allow(dead_code)]
            device_name: String,
        }

        let _: BrokenMember = from_candid(&stored_member().to_bytes());
    }

    #[test]
    fn check_fails_without_a_configuration() {
        assert!(check().unwrap_err().contains("no configuration"));
    }

    #[test]
    fn check_fails_without_a_policy() {
        set_config(config_value());
        assert!(check().unwrap_err().contains("no policy"));
    }

    #[test]
    fn the_migration_adds_the_default_policy_and_keeps_a_stored_one() {
        set_config(config_value());
        migrate(None).unwrap();
        assert_eq!(policy(), default_policy());
        assert_eq!(check(), Ok(()));

        let mut stricter = default_policy();
        stricter.rules[0].timelock_ns = 1;
        set_policy(stricter.clone());
        migrate(None).unwrap();
        assert_eq!(policy(), stricter);
    }

    fn ledger(seed: u8) -> Principal {
        Principal::from_slice(&[0, 0, 0, 0, 0, 0, 9, seed, 1, 1])
    }

    /// An account as a release before kinds stored it.
    fn legacy_account(chain: Chain, index: u32) -> StoredAccount {
        StoredAccount {
            kind: None,
            chain: Some(chain),
            ..StoredAccount::new(
                AccountKind::InternetComputer,
                "old".to_string(),
                index,
                None,
                Some(Principal::self_authenticating([3; 32])),
                0,
            )
        }
    }

    fn transfer_from(from: AccountId) -> StoredPending {
        let mut pending = action_at(5);
        pending.action = Action::Transfer(crate::TransferRequest {
            from,
            asset: None,
            to: "someone".to_string(),
            amount: Nat::from(5u8),
            fee: None,
            memo: None,
        });
        pending
    }

    #[test]
    fn the_migration_gives_old_accounts_kinds_and_old_transfers_their_asset() {
        let mut config = config_value();
        config.bitcoin_network = None;
        set_config(config);
        let icp = add_account(legacy_account(Chain::Icp, 0));
        let token = add_account(legacy_account(Chain::Icrc(ledger(1)), 0));
        let bitcoin = add_account(legacy_account(Chain::Bitcoin(BitcoinNetwork::Testnet), 0));
        let other = add_account(legacy_account(Chain::Icrc(ledger(2)), 0));
        let second = add_account(legacy_account(Chain::Icrc(ledger(1)), 1));
        let open = add_action(transfer_from(token));
        let mut done = transfer_from(other);
        done.state = ActionState::Executed(Outcome::Sent(crate::TxReceipt {
            account: other,
            chain: Chain::Icrc(ledger(2)),
            tx_id: "7".to_string(),
            at_ns: 5,
        }));
        let done = add_action(done);
        add_transfer(StoredTransfer {
            account: other,
            ..sent(Chain::Icrc(ledger(2)), 5, None, 5)
        });

        // An account stored before kinds fixes the network too.
        assert_eq!(fixed_bitcoin_network(), Some(BitcoinNetwork::Testnet));
        migrate(None).unwrap();
        assert_eq!(check(), Ok(()));
        assert_eq!(bitcoin_network(), BitcoinNetwork::Testnet);
        assert_eq!(
            tracked(),
            [Chain::Icp, Chain::Icrc(ledger(1)), Chain::Icrc(ledger(2))]
        );
        let kinds: Vec<(AccountId, Option<AccountKind>, Option<AccountId>)> = accounts()
            .into_iter()
            .map(|(id, account)| (id, account.kind, account.merged_into))
            .collect();
        let ic = Some(AccountKind::InternetComputer);
        assert_eq!(
            kinds,
            [
                (icp, ic, None),
                (token, ic, Some(icp)),
                (
                    bitcoin,
                    Some(AccountKind::Bitcoin(BitcoinNetwork::Testnet)),
                    None
                ),
                (other, ic, Some(icp)),
                (second, ic, None),
            ]
        );
        assert_eq!(listed_account(token).map(|(id, _)| id), Some(icp));
        let Action::Transfer(request) = action(open).unwrap().action else {
            panic!("a transfer");
        };
        assert_eq!(
            (request.from, request.asset),
            (icp, Some(Chain::Icrc(ledger(1))))
        );
        let migrated = action(done).unwrap();
        let ActionState::Executed(Outcome::Sent(receipt)) = migrated.state else {
            panic!("executed");
        };
        assert_eq!(receipt.account, icp);
        assert_eq!(
            HISTORY.with_borrow(|history| history.get(&0).unwrap().account),
            icp
        );

        // A second run changes nothing.
        let before = (accounts(), action(open), tracked());
        migrate(Some(BitcoinNetwork::Regtest)).unwrap();
        assert_eq!((accounts(), action(open), tracked()), before);
        assert_eq!(bitcoin_network(), BitcoinNetwork::Testnet);
    }

    #[test]
    fn the_upgrade_argument_sets_the_network_of_a_wallet_without_one() {
        let mut config = config_value();
        config.bitcoin_network = None;
        set_config(config);
        add_account(legacy_account(Chain::Icp, 0));
        migrate(Some(BitcoinNetwork::Regtest)).unwrap();
        assert_eq!(bitcoin_network(), BitcoinNetwork::Regtest);
    }

    #[test]
    fn a_wallet_without_a_network_or_bitcoin_account_is_on_mainnet() {
        let mut config = config_value();
        config.bitcoin_network = None;
        set_config(config);
        migrate(None).unwrap();
        assert_eq!(bitcoin_network(), BitcoinNetwork::Mainnet);
        assert_eq!(check(), Ok(()));
    }

    #[test]
    fn the_migration_watches_transactions_sent_before_the_wallet_watched_them() {
        set_config(config_value());
        let (mut sent, keys) = bitcoin_transfer(&[1]);
        sent.bitcoin_sending(1);
        sent.end_bitcoin_send(
            Ok(()),
            1,
            Chain::Bitcoin(BitcoinNetwork::Regtest),
            "t".to_string(),
            1,
        )
        .unwrap();
        let id = add_action(sent.clone());
        PENDING.with_borrow_mut(|actions| actions.insert(id, sent));
        reserve(&keys, id);
        assert!(unsettled().is_empty());
        migrate(None).unwrap();
        assert_eq!(unsettled(), [id]);
        assert_eq!(check(), Ok(()));
    }

    #[test]
    fn check_fails_when_a_merged_account_leads_nowhere() {
        set_config(config_value());
        set_policy(default_policy());
        set_tracked(assets::initial());
        let mut merged = StoredAccount::new(
            AccountKind::InternetComputer,
            "x".to_string(),
            0,
            None,
            None,
            0,
        );
        merged.merged_into = Some(7);
        add_account(merged);
        assert!(check().unwrap_err().contains("leads to account 7"));
    }

    #[test]
    fn check_passes_once_configured_and_reads_the_members() {
        set_config(config_value());
        set_policy(default_policy());
        set_tracked(assets::initial());
        insert_member(Principal::self_authenticating([3; 32]), stored_member());
        add_action(action_at(5));
        assert_eq!(check(), Ok(()));
        assert_eq!(config(), Some(config_value()));
        assert!(is_claimed());
    }

    #[test]
    fn members_are_read_back_with_their_principal() {
        let (first, second) = (
            Principal::self_authenticating([3; 32]),
            Principal::self_authenticating([4; 32]),
        );
        assert_eq!(member(first), None);
        assert_eq!(members(), vec![]);

        insert_member(first, stored_member());
        insert_member(second, stored_member());
        assert_eq!(member(first), Some(stored_member()));
        let mut expected = vec![
            stored_member().into_member(first),
            stored_member().into_member(second),
        ];
        expected.sort_by_key(|member| member.id);
        assert_eq!(members(), expected);
    }

    fn action_at(proposed_at_ns: u64) -> StoredPending {
        StoredPending::new(
            Action::RemoveMember(Principal::self_authenticating([3; 32])),
            Principal::self_authenticating([3; 32]),
            vec![],
            Terms {
                quorum: 1,
                timelock_ns: 0,
                daily_limit: false,
            },
            APPROVAL_WINDOW_NS,
            proposed_at_ns,
        )
    }

    #[test]
    fn every_ledger_call_gets_its_own_created_at_time() {
        assert_eq!(transfer_time(100), 100);
        // Messages of one round share their time.
        assert_eq!(transfer_time(100), 101);
        assert_eq!(transfer_time(100), 102);
        assert_eq!(transfer_time(50), 103);
        assert_eq!(transfer_time(1_000), 1_000);
    }

    #[test]
    fn action_ids_start_at_one_and_grow() {
        assert_eq!(add_action(action_at(1)), 1);
        assert_eq!(add_action(action_at(2)), 2);
        assert_eq!(action(2), Some(action_at(2)));
        assert_eq!(action(3), None);
    }

    #[test]
    fn open_actions_are_the_waiting_and_ready_ones_not_yet_expired() {
        let now = 30 * APPROVAL_WINDOW_NS;
        let expired = add_action(action_at(now - APPROVAL_WINDOW_NS));
        let open = add_action(action_at(now - 1));
        let rejected = add_action(action_at(now));
        let mut closed = action_at(now);
        closed.state = ActionState::Rejected(Principal::anonymous());
        set_action(rejected, closed);
        let newest = add_action(action_at(now));

        let ids = |open: Vec<(ActionId, StoredPending)>| -> Vec<ActionId> {
            open.into_iter().map(|(id, _)| id).collect()
        };
        assert_eq!(ids(open_actions(now)), [open, newest]);
        assert_eq!(OPEN.with_borrow(|open| open.len()), 3);

        close_expired(now);
        assert_eq!(action(expired).unwrap().state, ActionState::Expired);
        assert_eq!(
            OPEN.with_borrow(|open| open.keys().collect::<Vec<_>>()),
            [open, newest]
        );
        assert_eq!(ids(open_actions(now)), [open, newest]);
    }

    #[test]
    fn check_fails_when_an_open_id_has_no_action() {
        set_config(config_value());
        set_policy(default_policy());
        set_tracked(assets::initial());
        OPEN.with_borrow_mut(|open| open.insert(3, ()));
        assert_eq!(check(), Err("open action 3 is not stored".to_string()));
    }

    #[test]
    fn saving_governance_writes_members_and_policy() {
        let (kept, removed, added) = (
            Principal::self_authenticating([3; 32]),
            Principal::self_authenticating([4; 32]),
            Principal::self_authenticating([5; 32]),
        );
        set_policy(default_policy());
        insert_member(kept, stored_member());
        insert_member(removed, stored_member());

        let mut governance = governance();
        governance.members.remove(&removed);
        governance.members.insert(added, stored_member());
        governance.policy.rules[0].quorum = 2;
        save_governance(&governance);

        assert_eq!(member(removed), None);
        assert_eq!(member(added), Some(stored_member()));
        assert_eq!(member(kept), Some(stored_member()));
        assert_eq!(policy(), governance.policy);
        assert_eq!(super::governance(), governance);
    }

    fn entry(at_ns: u64) -> StoredAuditEntry {
        StoredAuditEntry {
            at_ns,
            by: Some(Principal::self_authenticating([3; 32])),
            event: AuditEvent::Proposed(at_ns),
        }
    }

    #[test]
    fn the_audit_log_pages_back_from_the_newest() {
        assert!(audit_page(None, 10).is_empty());
        for at_ns in 0..5 {
            assert_eq!(append_audit(entry(at_ns)), at_ns);
        }
        let seqs = |page: Vec<AuditEntry>| -> Vec<u64> { page.iter().map(|e| e.seq).collect() };
        assert_eq!(seqs(audit_page(None, 2)), [4, 3]);
        assert_eq!(seqs(audit_page(Some(3), 2)), [2, 1]);
        assert_eq!(seqs(audit_page(Some(1), 5)), [0]);
        assert!(audit_page(Some(0), 5).is_empty());
        assert_eq!(seqs(audit_page(Some(99), 1)), [4]);
        assert!(audit_page(None, 0).is_empty());
        let newest = &audit_page(None, 1)[0];
        assert_eq!(newest.at_ns, 4);
        assert_eq!(newest.event, AuditEvent::Proposed(4));
    }

    fn sent(chain: Chain, amount: u64, fee: Option<u64>, at_ns: u64) -> StoredTransfer {
        StoredTransfer {
            account: 1,
            chain,
            to: "someone".to_string(),
            amount: Nat::from(amount),
            tx_id: "1".to_string(),
            at_ns,
            action: 1,
            fee: fee.map(Nat::from),
        }
    }

    #[test]
    fn the_spending_of_a_chain_counts_bitcoin_fees() {
        let bitcoin = Chain::Bitcoin(crate::BitcoinNetwork::Regtest);
        add_transfer(sent(bitcoin.clone(), 1_000, Some(200), 10));
        add_transfer(sent(Chain::Icp, 5, None, 20));
        add_transfer(sent(bitcoin.clone(), 3_000, Some(400), 30));
        assert_eq!(sent_since(&bitcoin, 0), Nat::from(4_600u32));
        assert_eq!(sent_since(&bitcoin, 30), Nat::from(3_400u32));
        assert_eq!(sent_since(&Chain::Icp, 0), Nat::from(5u8));
    }

    /// A transfer whose transaction spends the outputs `seeds`.
    fn bitcoin_transfer(seeds: &[u8]) -> (StoredPending, Vec<[u8; 36]>) {
        let coins: Vec<btc::Coin> = seeds
            .iter()
            .map(|seed| btc::Coin {
                txid: [*seed; 32],
                vout: u32::from(*seed),
                value: 10_000,
                height: 1,
                mature: true,
            })
            .collect();
        let script = bitcoin::ScriptBuf::new_op_return([]);
        let plan = btc::Plan {
            inputs: coins.clone(),
            fee: 1_000,
            change: 0,
        };
        let tx = btc::unsigned(&plan, 5_000, &script, &script);
        let mut pending = action_at(1);
        pending.state = ActionState::Ready;
        pending.begin_bitcoin(1, 1).unwrap();
        pending.bitcoin_built(crate::actions::BitcoinTx::new(
            btc::to_bytes(&tx),
            coins.iter().map(|coin| coin.value).collect(),
            1_000,
        ));
        (pending, coins.iter().map(btc::Coin::key).collect())
    }

    #[test]
    fn a_closed_transfer_releases_its_outputs_unless_its_transaction_may_have_been_sent() {
        let (mut unsent, unsent_keys) = bitcoin_transfer(&[1, 2]);
        let (mut sent, sent_keys) = bitcoin_transfer(&[3]);
        let unsent_id = add_action(unsent.clone());
        let sent_id = add_action(sent.clone());
        reserve(&unsent_keys, unsent_id);
        reserve(&sent_keys, sent_id);
        // Another action's reservation of the same output stays.
        let (_, other_keys) = bitcoin_transfer(&[2]);
        assert_eq!(other_keys[0], unsent_keys[1]);

        sent.bitcoin_sending(1);
        sent.finish(Err("stopped".to_string()));
        set_action(sent_id, sent);
        assert!(sent_keys.iter().all(is_reserved));

        unsent.state = ActionState::Rejected(Principal::anonymous());
        reserve(&other_keys, 99);
        set_action(unsent_id, unsent);
        assert!(!is_reserved(&unsent_keys[0]));
        assert!(is_reserved(&unsent_keys[1]), "action 99 holds it now");
    }

    #[test]
    fn a_sent_transaction_is_watched_until_it_settles_and_then_releases_its_outputs() {
        let (mut pending, keys) = bitcoin_transfer(&[4, 5]);
        let id = add_action(pending.clone());
        reserve(&keys, id);
        set_action(id, pending.clone());
        assert!(unsettled().is_empty(), "built, not sent");

        pending.bitcoin_sending(1);
        set_action(id, pending.clone());
        assert_eq!(unsettled(), [id], "it may have been sent");
        let bitcoin = Chain::Bitcoin(crate::BitcoinNetwork::Regtest);
        pending
            .end_bitcoin_send(Ok(()), 1, bitcoin.clone(), "t".to_string(), 2)
            .unwrap();
        set_action(id, pending.clone());
        assert_eq!(unsettled(), [id]);
        assert!(keys.iter().all(is_reserved));

        assert!(!pending.bitcoin_settled(1, bitcoin, "t".to_string(), 3));
        set_action(id, pending);
        assert!(unsettled().is_empty());
        assert!(!keys.iter().any(is_reserved));
    }

    #[test]
    fn a_ledger_call_that_may_have_run_is_settled_while_the_transfer_is_open() {
        let mut pending = transfer_from(1);
        pending.state = ActionState::Ready;
        let id = add_action(pending.clone());
        pending.begin_transfer(10, || 10);
        set_action(id, pending.clone());
        pending.lose_transfer_call();
        set_action(id, pending.clone());
        assert_eq!(unsettled(), [id]);
        pending.finish(Err("too old".to_string()));
        set_action(id, pending);
        assert!(unsettled().is_empty());
    }

    #[test]
    fn the_migration_settles_what_an_older_release_left_unknown() {
        set_config(config_value());
        // A ledger call and a Bitcoin send that may have run, as a release
        // that did not settle them on its own stored them.
        let mut ledger = transfer_from(1);
        ledger.state = ActionState::Ready;
        let ledger_id = add_action(ledger.clone());
        ledger.begin_transfer(10, || 10);
        ledger.lose_transfer_call();
        set_action(ledger_id, ledger);
        let (mut bitcoin, keys) = bitcoin_transfer(&[6]);
        let bitcoin_id = add_action(bitcoin.clone());
        reserve(&keys, bitcoin_id);
        bitcoin.bitcoin_sending(1);
        set_action(bitcoin_id, bitcoin);
        for id in unsettled() {
            UNSETTLED.with_borrow_mut(|unsettled| unsettled.remove(&id));
        }

        migrate(None).unwrap();
        let mut settling = unsettled();
        settling.sort();
        assert_eq!(settling, [ledger_id, bitcoin_id]);
    }

    #[test]
    fn an_upgrade_sets_the_bitcoin_network_until_the_wallet_has_a_bitcoin_account() {
        set_config(config_value());
        assert_eq!(fixed_bitcoin_network(), None);
        migrate(Some(BitcoinNetwork::Testnet)).unwrap();
        assert_eq!(bitcoin_network(), BitcoinNetwork::Testnet);
        // Without a network, it keeps its own.
        migrate(None).unwrap();
        assert_eq!(bitcoin_network(), BitcoinNetwork::Testnet);
        add_account(StoredAccount::new(
            AccountKind::Bitcoin(BitcoinNetwork::Testnet),
            "Main".to_string(),
            0,
            None,
            None,
            0,
        ));
        assert_eq!(fixed_bitcoin_network(), Some(BitcoinNetwork::Testnet));
        migrate(None).unwrap();
        assert_eq!(bitcoin_network(), BitcoinNetwork::Testnet);
    }
}
