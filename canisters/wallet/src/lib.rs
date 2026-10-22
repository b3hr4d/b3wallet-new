//! Firmware of a self-owned chain-key wallet canister.
//!
//! The install arguments, `status`, state in stable memory, the claim,
//! members, the policy with its pending actions, the audit log, the
//! certified pages, the accounts of each kind with their balances and
//! transfers, and the self-upgrade to a signed release. The interface test
//! lists the methods of `candid/wallet.did` not implemented yet.

mod access;
mod accounts;
mod actions;
mod assets;
mod bitcoin_canister;
mod btc;
mod calls;
mod channel;
mod claim;
mod ed25519;
mod evm;
mod evm_rpc;
mod governance;
mod http;
mod icrc;
mod keys;
mod management;
mod policy;
mod sending;
mod storage;
mod types;
mod upgrade;

pub use access::{NOT_A_MEMBER, NOT_A_PROPOSER, NOT_AN_APPROVER, NOT_AN_OWNER, NOT_CLAIMED};
pub use claim::message as claim_message;
pub use types::*;
pub use upgrade::{FIRST_RESEND_NS, MAX_INSTALL_SENDS, cycles_to_upgrade};

use accounts::StoredAccount;
use actions::{
    APPROVAL_WINDOW_NS, JOIN_WINDOW_NS, MAX_OPEN_ACTIONS, MAX_OPEN_BY_PROPOSER,
    MAX_OPEN_JOIN_REQUESTS, StoredPending,
};
use candid::{Nat, Principal};
use policy::Spend;
use release::ReleaseManifest;
use serde_bytes::ByteBuf;
use std::cell::Cell;
use storage::{StoredAuditEntry, StoredMember};

#[ic_cdk::init]
fn init(arg: WalletArg) {
    let WalletArg::Init(args) = arg else {
        ic_cdk::trap("install needs WalletArg::Init");
    };
    let now = ic_cdk::api::time();
    if let Err(reason) = args.validate(now) {
        ic_cdk::trap(format!("invalid InitArgs: {reason}"));
    }
    storage::set_config(args.into());
    storage::set_policy(policy::default_policy());
    storage::set_tracked(assets::initial(storage::bitcoin_network()));
    add_defaults_now(now);
    http::certify();
    schedule_timer(now);
}

// A trap here makes the IC keep the running version: an upgrade either reads
// all of the state or does not happen.
#[ic_cdk::post_upgrade]
fn post_upgrade(arg: WalletArg) {
    let WalletArg::Upgrade(args) = arg else {
        ic_cdk::trap("an upgrade needs WalletArg::Upgrade");
    };
    let network = args.and_then(|args| args.bitcoin_network);
    if let Some(fixed) = storage::fixed_bitcoin_network()
        && network.is_some_and(|asked| asked != fixed)
    {
        ic_cdk::trap(format!(
            "the wallet's Bitcoin network is {fixed:?}, and its Bitcoin and EVM accounts keep it"
        ));
    }
    if let Err(reason) = storage::migrate(network).and_then(|()| storage::check()) {
        ic_cdk::trap(format!("the upgrade cannot read the state: {reason}"));
    }
    let now = ic_cdk::api::time();
    add_defaults_now(now);
    http::certify();
    // This version runs, so an install the wallet sent itself ran.
    let mut upgrade = storage::upgrade();
    if let Some((id, version)) = upgrade.installed() {
        storage::set_upgrade(upgrade);
        end_upgrade(
            id,
            Ok(Outcome::Done),
            AuditEvent::UpgradeInstalled(version),
            now,
        );
    }
    // An upgrade turns the timer off.
    schedule_timer(now);
}

/// Makes the caller the first owner. See `claim::check` for what it refuses.
#[ic_cdk::update]
fn claim(args: ClaimArgs) -> Result<Member, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let now = ic_cdk::api::time();
    let Some(config) = storage::config() else {
        ic_cdk::trap("the wallet has no configuration");
    };
    claim::check(
        config.claim.as_ref(),
        storage::is_claimed(),
        ic_cdk::api::canister_self(),
        caller,
        now,
        &args.signature,
    )?;
    let member = StoredMember {
        role: Role::Owner,
        label: "first owner".to_string(),
        added_at_ns: now,
        added_by: None,
    };
    storage::complete_claim(caller, member.clone());
    audit(caller, AuditEvent::Claimed, now);
    Ok(member.into_member(caller))
}

/// The caller's membership; `None` for anyone who is not a member.
#[ic_cdk::query(guard = "access::claimed")]
fn whoami() -> Option<Member> {
    let caller = ic_cdk::api::msg_caller();
    storage::member(caller).map(|member| member.into_member(caller))
}

#[ic_cdk::query(guard = "access::member")]
fn members() -> Result<Vec<Member>, WalletError> {
    Ok(storage::members())
}

#[ic_cdk::query(guard = "access::member")]
fn policy() -> Result<Policy, WalletError> {
    Ok(storage::policy())
}

/// The open actions, oldest first.
#[ic_cdk::query(guard = "access::member")]
fn pending() -> Result<Vec<Pending>, WalletError> {
    let now = ic_cdk::api::time();
    Ok(storage::open_actions(now)
        .into_iter()
        .map(|(id, pending)| pending.view(id, now))
        .collect())
}

/// The wallet's accounts, in id order.
#[ic_cdk::query(guard = "access::member")]
fn accounts() -> Result<Vec<Account>, WalletError> {
    let wallet = ic_cdk::api::canister_self();
    Ok(storage::accounts()
        .into_iter()
        .filter(|(_, account)| account.is_listed())
        .map(|(id, account)| account.view(id, wallet))
        .collect())
}

/// Adds an account: the next index of its kind, held by the wallet. Adding
/// one moves nothing, so it needs no approval. A Bitcoin or EVM account asks
/// the management canister for the public key of its derivation path first.
#[ic_cdk::update(guard = "access::owner")]
async fn add_account(args: AddAccountArgs) -> Result<Account, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    check_not_installing(ic_cdk::api::time())?;
    let index = accounts::check_new(&args, storage::accounts().iter().map(|(_, a)| a))?;
    let public_key = account_key(args.kind, index).await?;
    if public_key.is_some() {
        // Other calls ran meanwhile.
        if storage::member(caller).map(|member| member.role) != Some(Role::Owner) {
            return Err(WalletError::Forbidden(
                "the caller is no longer an owner".to_string(),
            ));
        }
        let again = accounts::check_new(&args, storage::accounts().iter().map(|(_, a)| a))?;
        if again != index {
            return Err(WalletError::InvalidArgument(
                "another account of this kind was added meanwhile; add this one again".to_string(),
            ));
        }
    }
    let now = ic_cdk::api::time();
    let account = StoredAccount::new(args.kind, args.label, index, public_key, Some(caller), now);
    let id = storage::add_account(account.clone());
    audit(caller, AuditEvent::AccountAdded(id), now);
    Ok(account.view(id, ic_cdk::api::canister_self()))
}

/// The public key of account `index` of `kind`, checked to make an
/// address; an Internet Computer account has none. An EVM account's key is
/// under the subnet key of the wallet's network.
async fn account_key(kind: AccountKind, index: u32) -> Result<Option<Vec<u8>>, WalletError> {
    let path = match kind {
        AccountKind::InternetComputer => return Ok(None),
        AccountKind::Bitcoin(network) => keys::bitcoin(network, index),
        AccountKind::Evm => keys::evm(storage::bitcoin_network(), index),
    };
    let key = keys::public_key(&path)
        .await
        .map_err(WalletError::ChainError)?;
    let checked = match kind {
        AccountKind::Bitcoin(network) => btc::address(&key, network).map(drop),
        _ => evm::address(&key).map(drop),
    };
    checked.map_err(WalletError::ChainError)?;
    Ok(Some(key))
}

thread_local! {
    /// Whether the wallet has account 0 of each kind. On the heap: a new
    /// version checks again.
    static DEFAULTS_DONE: Cell<bool> = const { Cell::new(false) };
    /// Whether the timer is adding a default account now.
    static ADDING_DEFAULTS: Cell<bool> = const { Cell::new(false) };
    /// When the timer next tries to add a default account, and how long it
    /// waited last.
    static DEFAULTS_RETRY: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// How long the timer first waits to try a default account again; each
/// later wait doubles, up to a day.
const DEFAULTS_FIRST_WAIT_NS: u64 = 60 * 1_000_000_000;

/// The default accounts the wallet lacks: account 0 of each kind, held by
/// the wallet. The heap remembers once there are none.
fn missing_defaults() -> Vec<AccountKind> {
    if DEFAULTS_DONE.get() {
        return Vec::new();
    }
    let accounts = storage::accounts();
    let missing =
        accounts::missing_defaults(storage::bitcoin_network(), accounts.iter().map(|(_, a)| a));
    DEFAULTS_DONE.set(missing.is_empty());
    missing
}

/// Stores default account `kind`, made by the wallet, unless another
/// account of that kind took index 0 meanwhile. The audit log has no entry
/// for it: every wallet has its default accounts, and nobody chose them.
fn add_default(kind: AccountKind, public_key: Option<Vec<u8>>, now: u64) {
    let accounts = storage::accounts();
    if accounts::next_index(kind, accounts.iter().map(|(_, a)| a)) != 0 {
        return;
    }
    let account = StoredAccount::new(
        kind,
        accounts::DEFAULT_LABEL.to_string(),
        0,
        public_key,
        None,
        now,
    );
    storage::add_account(account);
}

/// Adds the default accounts that need no call: the Internet Computer one.
fn add_defaults_now(now: u64) {
    if missing_defaults().contains(&AccountKind::InternetComputer) {
        add_default(AccountKind::InternetComputer, None, now);
    }
}

/// When the timer next tries to add a default account.
fn defaults_at() -> Option<u64> {
    if ADDING_DEFAULTS.get() || missing_defaults().is_empty() {
        return None;
    }
    Some(DEFAULTS_RETRY.get().0)
}

/// Adds the default Bitcoin and EVM accounts, which need their public keys
/// from the management canister. A failure waits, longer each time, and
/// tries again; the keys come from the same subnet, so the others wait too.
async fn add_defaults() {
    if ADDING_DEFAULTS.replace(true) {
        return;
    }
    struct Adding;
    impl Drop for Adding {
        fn drop(&mut self) {
            ADDING_DEFAULTS.set(false);
            schedule_timer(ic_cdk::api::time());
        }
    }
    let _adding = Adding;
    add_defaults_now(ic_cdk::api::time());
    for kind in missing_defaults() {
        if kind == AccountKind::InternetComputer {
            continue;
        }
        let key = account_key(kind, 0).await;
        let now = ic_cdk::api::time();
        match key {
            Ok(key) if missing_defaults().contains(&kind) => add_default(kind, key, now),
            Ok(_) => {}
            Err(_) => {
                let (_, waited) = DEFAULTS_RETRY.get();
                let wait = waited
                    .saturating_mul(2)
                    .clamp(DEFAULTS_FIRST_WAIT_NS, policy::DAY_NS);
                DEFAULTS_RETRY.set((now.saturating_add(wait), wait));
                break;
            }
        }
    }
}

/// What an account holds of an asset, as its ledger, the Bitcoin canister or
/// an EVM chain's providers answer it now: `asset`, or ICP, or the account's
/// bitcoin. A ledger asset or EVM chain must be tracked, so no member makes
/// the wallet call a canister the owners did not choose. A Bitcoin balance
/// counts outputs with at least one confirmation, less what the wallet's
/// transactions spend, plus their change. An EVM balance is the chain's at
/// its latest block, in wei.
#[ic_cdk::update(guard = "access::member")]
async fn balance(id: AccountId, asset: Option<Chain>) -> Result<Balance, WalletError> {
    check_not_installing(ic_cdk::api::time())?;
    let (id, account) = storage::listed_account(id)
        .ok_or_else(|| WalletError::NotFound(format!("account {id}")))?;
    let asset = assets::resolve(account.kind(), asset.as_ref(), &storage::tracked())?;
    let amount = match (account.kind(), assets::ledger(&asset)) {
        (AccountKind::InternetComputer, Some(ledger)) => {
            let owner = account.ledger_account(ic_cdk::api::canister_self());
            icrc::balance_of(ledger, owner)
                .await
                .map_err(WalletError::ChainError)?
        }
        (AccountKind::Bitcoin(network), _) => {
            Nat::from(sending::bitcoin_balance(id, &account, network).await?)
        }
        (AccountKind::Evm, _) => {
            let Chain::Evm(chain) = asset else {
                unreachable!("an EVM account holds EVM chains");
            };
            evm_rpc::balance(chain, &account.evm_address()?)
                .await
                .map_err(WalletError::ChainError)?
        }
        (kind, _) => {
            return Err(WalletError::InvalidArgument(format!(
                "a {kind:?} account has no balance of {asset:?} yet"
            )));
        }
    };
    Ok(Balance {
        account: id,
        asset,
        amount,
        as_of_ns: ic_cdk::api::time(),
    })
}

/// The assets the accounts show, ICP first.
#[ic_cdk::query(guard = "access::member")]
fn assets() -> Result<Vec<Chain>, WalletError> {
    Ok(storage::tracked())
}

/// Tracks an ICRC-1 ledger, once it answers as one, or an EVM chain the
/// wallet's network holds. Tracking moves nothing, so it needs no approval.
#[ic_cdk::update(guard = "access::owner")]
async fn track_asset(asset: Chain) -> Result<Vec<Chain>, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    check_not_installing(ic_cdk::api::time())?;
    assets::check_track(&asset, &storage::tracked(), storage::bitcoin_network())?;
    if let Some(ledger) = assets::ledger(&asset) {
        icrc::check_ledger(ledger)
            .await
            .map_err(WalletError::ChainError)?;
        // Other calls ran meanwhile.
        if storage::member(caller).map(|member| member.role) != Some(Role::Owner) {
            return Err(WalletError::Forbidden(
                "the caller is no longer an owner".to_string(),
            ));
        }
    }
    let mut tracked = storage::tracked();
    assets::check_track(&asset, &tracked, storage::bitcoin_network())?;
    tracked.push(asset.clone());
    storage::set_tracked(tracked.clone());
    audit(caller, AuditEvent::AssetTracked(asset), ic_cdk::api::time());
    Ok(tracked)
}

/// Stops showing an ICRC-1 token or an EVM chain. Its tokens stay where they
/// are, and its open transfers still run.
#[ic_cdk::update(guard = "access::owner")]
fn untrack_asset(asset: Chain) -> Result<Vec<Chain>, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let mut tracked = storage::tracked();
    assets::check_untrack(&asset, &tracked)?;
    tracked.retain(|tracked| *tracked != asset);
    storage::set_tracked(tracked.clone());
    audit(
        caller,
        AuditEvent::AssetUntracked(asset),
        ic_cdk::api::time(),
    );
    Ok(tracked)
}

/// Proposes an action. An owner's approval counts toward its quorum, so
/// with a quorum of one it is ready at once. A transfer that keeps its
/// asset's spending within the daily limit is ready at once too. A proposer
/// proposes only transfers, and its approval never counts: each one waits
/// for an owner or an approver. A transfer is stored with the asset it
/// sends, and the listed account it sends from.
#[ic_cdk::update(guard = "access::proposer")]
fn propose(action: Action) -> Result<Pending, WalletError> {
    let mut action = action;
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let governance = storage::governance();
    let approves = governance.can_approve(&caller);
    if !approves && !matches!(action, Action::Transfer(_)) {
        return Err(WalletError::Forbidden(
            "a proposer proposes only transfers".to_string(),
        ));
    }
    let spend = match &mut action {
        Action::Transfer(request) => {
            let (from, account) = storage::listed_account(request.from)
                .ok_or_else(|| WalletError::NotFound(format!("account {}", request.from)))?;
            let asset =
                assets::resolve(account.kind(), request.asset.as_ref(), &storage::tracked())?;
            request.from = from;
            request.asset = Some(asset.clone());
            let amount = sending::check_transfer(request, &account)?;
            Some((asset, amount))
        }
        Action::Upgrade {
            version,
            wasm_sha256,
        } => {
            upgrade::check_action(version, wasm_sha256)?;
            None
        }
        _ => {
            governance.check(&action, caller, now)?;
            None
        }
    };
    storage::close_expired(now);
    let open = storage::open_actions(now);
    if open.iter().filter(|(_, p)| !p.is_join_request()).count() >= MAX_OPEN_ACTIONS {
        return Err(WalletError::Forbidden(format!(
            "at most {MAX_OPEN_ACTIONS} actions can be open at once"
        )));
    }
    if !approves
        && open.iter().filter(|(_, p)| p.proposed_by == caller).count() >= MAX_OPEN_BY_PROPOSER
    {
        return Err(WalletError::Forbidden(format!(
            "a proposer can have at most {MAX_OPEN_BY_PROPOSER} actions open at once"
        )));
    }
    let terms = match &spend {
        Some((asset, amount)) => {
            let spent = sending::spent(asset, now, None);
            let spend = Spend {
                chain: asset,
                amount,
                spent: &spent,
            };
            policy::terms(&governance.policy, ActionKind::Transfer, Some(&spend))
        }
        None => policy::terms(&governance.policy, action.kind(), None),
    };
    let approvals = if approves { vec![caller] } else { vec![] };
    let pending = StoredPending::new(action, caller, approvals, terms, APPROVAL_WINDOW_NS, now);
    let id = storage::add_action(pending.clone());
    audit(caller, AuditEvent::Proposed(id), now);
    Ok(pending.view(id, now))
}

/// Asks to join as a viewer, or as a proposer when `role` says so. It is an
/// `AddMember` action that the members approve like any other. Asking again
/// while a request is open returns it.
#[ic_cdk::update(guard = "access::claimed")]
fn request_join(label: String, role: Option<Role>) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let role = role.unwrap_or(Role::Viewer);
    if !matches!(role, Role::Viewer | Role::Proposer) {
        return Err(WalletError::Forbidden(
            "a join request asks to be a viewer or a proposer".to_string(),
        ));
    }
    if caller == Principal::anonymous() {
        return Err(WalletError::Forbidden(
            "the anonymous principal cannot join".to_string(),
        ));
    }
    let governance = storage::governance();
    if governance.members.contains_key(&caller) {
        return Err(WalletError::Forbidden(
            "the caller is already a member".to_string(),
        ));
    }
    storage::close_expired(now);
    let open = storage::open_actions(now);
    let requests: Vec<&(ActionId, StoredPending)> =
        open.iter().filter(|(_, p)| p.is_join_request()).collect();
    if let Some((id, pending)) = requests.iter().find(|(_, p)| p.proposed_by == caller) {
        return Ok(pending.view(*id, now));
    }
    if requests.len() >= MAX_OPEN_JOIN_REQUESTS {
        return Err(WalletError::Forbidden(format!(
            "at most {MAX_OPEN_JOIN_REQUESTS} join requests can be open at once"
        )));
    }
    let action = Action::AddMember {
        id: caller,
        role,
        label,
    };
    governance.check(&action, caller, now)?;
    let terms = policy::terms(&governance.policy, ActionKind::AddMember, None);
    let pending = StoredPending::new(action, caller, vec![], terms, JOIN_WINDOW_NS, now);
    let id = storage::add_action(pending.clone());
    audit(caller, AuditEvent::JoinRequested(caller), now);
    Ok(pending.view(id, now))
}

#[ic_cdk::update(guard = "access::approver")]
fn approve(id: ActionId) -> Result<Pending, WalletError> {
    change_action(id, |pending, caller, now| {
        let before = pending.approvals.len();
        pending.approve(id, caller, now)?;
        // Approving twice changes nothing, so it logs nothing.
        if pending.approvals.len() > before {
            audit(caller, AuditEvent::Approved(id), now);
        }
        Ok(())
    })
}

#[ic_cdk::update(guard = "access::owner")]
fn reject(id: ActionId) -> Result<Pending, WalletError> {
    // An install past its deadline ends before its action can be rejected.
    settle_upgrade(ic_cdk::api::time());
    change_action(id, |pending, caller, now| {
        pending.reject(id, caller, now)?;
        audit(caller, AuditEvent::Rejected(id), now);
        // The release it staged will not be installed.
        let mut upgrade = storage::upgrade();
        if upgrade.drop_staging(id) {
            storage::set_upgrade(upgrade);
            schedule_timer(now);
        }
        Ok(())
    })
}

/// Runs an action that has its quorum once its timelock has passed. An
/// action that no longer fits the members or the policy ends `Failed`.
#[ic_cdk::update(guard = "access::approver")]
async fn execute(id: ActionId) -> Result<Pending, WalletError> {
    let now = ic_cdk::api::time();
    // Nothing runs while an install is on its way, nor a second upgrade.
    check_not_installing(now)?;
    let mut pending = stored_action(id)?;
    if let Err(error) = pending.check_executable(id, now) {
        storage::set_action(id, pending);
        return Err(error);
    }
    if let Action::Upgrade {
        version,
        wasm_sha256,
    } = pending.action.clone()
    {
        return execute_upgrade(id, pending, version, wasm_sha256, now).await;
    }
    if let Action::Transfer(request) = pending.action.clone() {
        let caller = ic_cdk::api::msg_caller();
        return sending::execute(id, pending, request, Some(caller), now).await;
    }
    let mut governance = storage::governance();
    // The first approval is the proposer's, or for a join request the first
    // member who approved it.
    let added_by = pending
        .approvals
        .first()
        .copied()
        .unwrap_or(pending.proposed_by);
    let result = governance.apply(&pending.action, added_by, now);
    if result.is_ok() {
        storage::save_governance(&governance);
        // An approval counts only while its member can approve.
        for (open_id, mut open) in storage::open_actions(now) {
            let before = open.approvals.len();
            for approver in open.approvals.clone() {
                if !governance.can_approve(&approver) {
                    open.drop_approval(approver);
                }
            }
            if open.approvals.len() != before {
                storage::set_action(open_id, open);
            }
        }
    }
    pending.finish(
        result
            .map(|()| Outcome::Done)
            .map_err(|error| reason(&error)),
    );
    storage::set_action(id, pending.clone());
    audit(ic_cdk::api::msg_caller(), AuditEvent::Executed(id), now);
    Ok(pending.view(id, now))
}

/// How a step of a Bitcoin transfer or an upgrade failed.
enum Step {
    /// The action cannot run: it ends `Failed` with this reason.
    Fails(String),
    /// This execution stops; the action stays ready.
    Stops(WalletError),
}

impl From<WalletError> for Step {
    fn from(error: WalletError) -> Self {
        Step::Stops(error)
    }
}

fn chain_error(reason: String) -> Step {
    Step::Stops(WalletError::ChainError(reason))
}

/// The release the channel published last, when the wallet would install
/// it: signed by enough of the release signer keys the wallet pins, and
/// tested to upgrade from the module the wallet runs. `None` when the
/// channel has no release, or the wallet runs its latest.
#[ic_cdk::update(guard = "access::member")]
async fn release_available() -> Result<Option<ReleaseManifest>, WalletError> {
    check_not_installing(ic_cdk::api::time())?;
    let config = installed_config();
    let latest = channel::latest(config.release_channel)
        .await
        .map_err(WalletError::ChainError)?;
    let Some(manifest) = latest else {
        return Ok(None);
    };
    let module_hash = management::itself()
        .await
        .map_err(WalletError::ChainError)?
        .module_hash;
    if module_hash.as_deref() == Some(manifest.wasm_sha256.as_slice()) {
        return Ok(None);
    }
    upgrade::check_release(&manifest, &release_signers(&config), module_hash.as_deref())
        .map_err(WalletError::ReleaseRejected)?;
    Ok(Some(manifest))
}

#[ic_cdk::query(guard = "access::member")]
fn upgrade_status() -> UpgradeState {
    storage::upgrade().view(ic_cdk::api::time())
}

fn installed_config() -> storage::Config {
    storage::config().unwrap_or_else(|| ic_cdk::trap("the wallet has no configuration"))
}

/// The release signer keys and threshold the wallet was installed with,
/// which `init` checked.
fn release_signers(config: &storage::Config) -> release::Signers {
    release::Signers::new(&config.release_signers, config.release_threshold)
        .unwrap_or_else(|reason| ic_cdk::trap(format!("the stored release signers: {reason}")))
}

/// Refuses while an install is on its way. An install whose deadline passed
/// failed: that is written down first, so a timer that never ran keeps
/// nothing waiting.
fn check_not_installing(now: u64) -> Result<(), WalletError> {
    settle_upgrade(now);
    if storage::upgrade().is_installing() {
        return Err(WalletError::UpgradeInProgress);
    }
    Ok(())
}

/// Ends an install the old version outlived past its deadline.
fn settle_upgrade(now: u64) {
    let mut upgrade = storage::upgrade();
    if let Some((id, version)) = upgrade.check_deadline(now) {
        storage::set_upgrade(upgrade);
        let reason = upgrade::failure(&version);
        end_upgrade(id, Err(reason), AuditEvent::UpgradeFailed(version), now);
        schedule_timer(now);
    }
}

/// Ends upgrade action `id` with `result`, and logs it and `event` as the
/// wallet's own doing.
fn end_upgrade(id: Option<ActionId>, result: Result<Outcome, String>, event: AuditEvent, now: u64) {
    if let Some(id) = id
        && let Some(mut pending) = storage::action(id)
    {
        pending.finish(result);
        storage::set_action(id, pending);
        audit_by_wallet(AuditEvent::Executed(id), now);
    }
    audit_by_wallet(event, now);
}

/// Executes an upgrade: checks the release the channel published under
/// `version` against the action and the wallet, stages its chunks in the
/// wallet's chunk store, and sends the install. One execution at a time; a
/// later one goes on where an earlier one stopped.
async fn execute_upgrade(
    id: ActionId,
    pending: StoredPending,
    version: String,
    wasm_sha256: ByteBuf,
    now: u64,
) -> Result<Pending, WalletError> {
    let caller = ic_cdk::api::msg_caller();
    let mut upgrade = storage::upgrade();
    let execution = upgrade.begin(now)?;
    storage::set_upgrade(upgrade);
    storage::set_action(id, pending);
    let in_flight = Staging(Some(execution));
    let result = upgrade_steps(id, execution, &version, &wasm_sha256, caller).await;

    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    let mut upgrade = storage::upgrade();
    let ours = upgrade.owns(execution);
    let result = match result {
        // The install is on its way.
        Ok(()) => Ok(()),
        // Nothing awaited since the step that failed, so the action is still
        // open and this execution's.
        Err(Step::Fails(reason)) if ours => {
            pending.finish(Err(reason));
            audit(caller, AuditEvent::Executed(id), now);
            Ok(())
        }
        Err(Step::Fails(reason)) => Err(WalletError::ReleaseRejected(reason)),
        Err(Step::Stops(error)) => Err(error),
    };
    // A release whose action failed, expired or was rejected meanwhile will
    // not be installed.
    if !pending.is_open(now) {
        upgrade.drop_staging(id);
    }
    upgrade.end(execution);
    storage::set_upgrade(upgrade);
    storage::set_action(id, pending.clone());
    schedule_timer(now);
    in_flight.returned();
    result.map(|()| pending.view(id, now))
}

/// Re-reads upgrade action `id` after a call and goes on with
/// `execution`, if the action may still run and the execution still owns
/// the staging.
fn resume_upgrade(id: ActionId, execution: u64) -> Result<upgrade::StoredUpgrade, Step> {
    let now = ic_cdk::api::time();
    let mut pending = stored_action(id)?;
    if let Err(error) = pending.check_executable(id, now) {
        storage::set_action(id, pending);
        return Err(Step::Stops(error));
    }
    let mut upgrade = storage::upgrade();
    upgrade.resume(execution, now)?;
    storage::set_upgrade(upgrade.clone());
    Ok(upgrade)
}

/// The steps of execution `execution` that call out, then the install.
async fn upgrade_steps(
    id: ActionId,
    execution: u64,
    version: &str,
    wasm_sha256: &[u8],
    caller: Principal,
) -> Result<(), Step> {
    let config = installed_config();
    let manifest = channel::manifest(config.release_channel, version)
        .await
        .map_err(chain_error)?;
    resume_upgrade(id, execution)?;
    let manifest = manifest.ok_or_else(|| {
        Step::Stops(WalletError::ReleaseRejected(format!(
            "the release channel has no release {version}"
        )))
    })?;
    let itself = management::itself().await.map_err(chain_error)?;
    resume_upgrade(id, execution)?;
    let module_hash = itself.module_hash;
    if manifest.version != version || manifest.wasm_sha256.as_slice() != wasm_sha256 {
        return Err(Step::Fails(format!(
            "release {version} on the release channel is not the Wasm the action names"
        )));
    }
    if module_hash.as_deref() == Some(wasm_sha256) {
        return Err(Step::Fails(format!(
            "the wallet already runs release {version}"
        )));
    }
    upgrade::check_release(&manifest, &release_signers(&config), module_hash.as_deref())
        .map_err(Step::Fails)?;
    if !itself.controls_itself {
        return Err(Step::Stops(WalletError::Forbidden(
            "the wallet is not its own controller, so the IC would refuse to install a release"
                .to_string(),
        )));
    }

    let total = u32::try_from(manifest.chunk_hashes.len()).expect("at most 100 chunks");
    let mut upgrade = storage::upgrade();
    let first = upgrade.stage(id, manifest.clone());
    // The IC refuses an install the wallet cannot pay for, and only the
    // one-way install's lost reply would hear it: stop before staging.
    enough_cycles(upgrade::cycles_to_upgrade(total - first))?;
    let clear = upgrade.clear_store;
    storage::set_upgrade(upgrade);
    if clear {
        management::clear_chunk_store().await.map_err(chain_error)?;
        let mut upgrade = resume_upgrade(id, execution)?;
        upgrade.clear_store = false;
        storage::set_upgrade(upgrade);
    }
    for index in first..total {
        let chunk = channel::chunk(config.release_channel, version, index)
            .await
            .map_err(chain_error)?;
        resume_upgrade(id, execution)?;
        let chunk = chunk.ok_or_else(|| {
            chain_error(format!(
                "the release channel has no chunk {index} of release {version}"
            ))
        })?;
        manifest
            .check_chunk(index as usize, &chunk)
            .map_err(|error| {
                Step::Fails(format!(
                    "the release channel served chunk {index} of release {version} out of \
                     place: {error:?}"
                ))
            })?;
        let stored = management::upload_chunk(chunk).await.map_err(chain_error)?;
        let mut upgrade = resume_upgrade(id, execution)?;
        if stored.as_slice() != manifest.chunk_hashes[index as usize].as_slice() {
            return Err(chain_error(format!(
                "the chunk store keeps chunk {index} under another hash"
            )));
        }
        upgrade.uploaded(index);
        storage::set_upgrade(upgrade);
        if index + 1 == total {
            audit(
                caller,
                AuditEvent::UpgradeStaged(version.to_string()),
                ic_cdk::api::time(),
            );
        }
    }

    // No await from here on: the install goes out with no call waiting.
    let now = ic_cdk::api::time();
    let mut upgrade = resume_upgrade(id, execution)?;
    let waiting = calls::in_flight();
    if waiting > 0 {
        return Err(Step::Stops(WalletError::InvalidArgument(format!(
            "release {version} is staged, but the wallet still waits for the answers to \
             {waiting} of its calls; execute the upgrade again once they return"
        ))));
    }
    enough_cycles(upgrade::INSTALL_CYCLES)?;
    let (chunk_hashes, module_hash) = match upgrade.install_hashes() {
        Some(hashes) if upgrade.is_staged() => hashes,
        _ => return Err(chain_error(format!("release {version} is not staged"))),
    };
    management::install(chunk_hashes, module_hash).map_err(chain_error)?;
    upgrade.installing(now);
    storage::set_upgrade(upgrade);
    let mut pending = stored_action(id)?;
    pending.install_sent();
    storage::set_action(id, pending);
    schedule_timer(now);
    Ok(())
}

/// Stops an upgrade unless the wallet can spend `needed` cycles above what it
/// keeps to stay running.
fn enough_cycles(needed: u128) -> Result<(), Step> {
    let available = ic_cdk::api::canister_liquid_cycle_balance();
    if available < needed {
        return Err(Step::Stops(WalletError::InsufficientCycles {
            available: Nat::from(available),
            needed: Nat::from(needed),
        }));
    }
    Ok(())
}

/// Marks an upgrade execution as one that will not go on, unless the code
/// after its calls returns: when that code traps, ic-cdk drops the call's
/// future, and with it this guard, and keeps what the guard writes.
struct Staging(Option<u64>);

impl Staging {
    fn returned(mut self) {
        self.0 = None;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(execution) = self.0 {
            let mut upgrade = storage::upgrade();
            upgrade.end(execution);
            storage::set_upgrade(upgrade);
        }
    }
}

thread_local! {
    /// When the timer may next try to clear the chunk store.
    static CLEAR_AT: Cell<u64> = const { Cell::new(0) };
}

/// Sets the IC's one timer for the next thing the wallet waits for, or
/// turns it off: the install it sent, clearing the chunk store, a default
/// account, and settling the transfers whose outcome it does not know yet.
/// While an install is on its way, the wallet makes no calls but those.
fn schedule_timer(now: u64) {
    let upgrade = storage::upgrade();
    let calls = !upgrade.is_installing();
    let upgrade_at = match upgrade.wake_at(now) {
        // It wakes only to clear the chunk store.
        Some(at) if calls => Some(at.max(CLEAR_AT.get())),
        at => at,
    };
    let at = [
        upgrade_at,
        calls.then(defaults_at).flatten(),
        calls.then(sending::settle_at).flatten(),
    ]
    .into_iter()
    .flatten()
    .min();
    // Zero turns the timer off; any time not after now fires it at once.
    ic_cdk::api::global_timer_set(at.map_or(0, |at| at.max(1)));
}

/// How often the timer tries again to clear the chunk store.
const CLEAR_RETRY_NS: u64 = 60 * 1_000_000_000;

/// Ends an install past its deadline, sends one again when its turn comes,
/// clears the chunk store once no release needs it, adds the default
/// accounts the wallet lacks, and settles transfers, each when it is due.
fn on_timer(now: u64) {
    settle_upgrade(now);
    resend_install(now);
    let mut upgrade = storage::upgrade();
    if upgrade.may_clear_store() && now >= CLEAR_AT.get() {
        if management::clear_chunk_store_oneway().is_ok() {
            upgrade.clear_store = false;
            storage::set_upgrade(upgrade.clone());
        } else {
            CLEAR_AT.set(now.saturating_add(CLEAR_RETRY_NS));
        }
    }
    if !upgrade.is_installing() {
        if defaults_at().is_some_and(|at| now >= at) {
            ic_cdk::futures::spawn(add_defaults());
        }
        if sending::settle_at().is_some_and(|at| now >= at) {
            ic_cdk::futures::spawn(sending::settle(now));
        }
    }
    schedule_timer(now);
}

/// Sends the install on its way again when its turn comes. This version
/// still runs, so the IC refused the last send, as it does for a while after
/// another install, or has not run it yet; then the new version installs
/// again and its `post_upgrade` finds the upgrade done. A turn passes unused
/// while a call waits for its answer or the wallet cannot pay for the
/// install.
fn resend_install(now: u64) {
    let mut upgrade = storage::upgrade();
    if !upgrade.resend(now) {
        return;
    }
    if calls::in_flight() == 0
        && ic_cdk::api::canister_liquid_cycle_balance() >= upgrade::INSTALL_CYCLES
        && let Some((chunk_hashes, module_hash)) = upgrade.install_hashes()
    {
        // A send that cannot go out passes like an unused turn.
        let _ = management::install(chunk_hashes, module_hash);
    }
    storage::set_upgrade(upgrade);
}

#[cfg_attr(target_family = "wasm", unsafe(export_name = "canister_global_timer"))]
#[cfg_attr(
    not(target_family = "wasm"),
    unsafe(export_name = "canister_global_timer_host")
)]
fn global_timer() {
    ic_cdk::futures::internals::in_executor_context(|| on_timer(ic_cdk::api::time()));
}

/// Newest first: up to `limit` entries, at most `MAX_AUDIT_PAGE`, before
/// `before_seq`, or from the newest.
#[ic_cdk::query(guard = "access::member")]
fn audit_log(request: AuditRequest) -> Result<Vec<AuditEntry>, WalletError> {
    let limit = usize::from(request.limit).min(MAX_AUDIT_PAGE);
    Ok(storage::audit_page(request.before_seq, limit))
}

/// The most entries one `audit_log` call returns.
const MAX_AUDIT_PAGE: usize = 100;

/// Records a change made by `by`. Every call that changes state writes one
/// entry for it; a refused call writes none. An expiry is no one's act and is
/// not logged: the action itself shows it.
fn audit(by: Principal, event: AuditEvent, now: u64) {
    log(Some(by), event, now);
}

/// Records a change the wallet made itself: how an upgrade ended, a default
/// account, or a transfer it settled.
fn audit_by_wallet(event: AuditEvent, now: u64) {
    log(None, event, now);
}

/// Records a change made by `by`, or by the wallet itself.
fn log(by: Option<Principal>, event: AuditEvent, now: u64) {
    storage::append_audit(StoredAuditEntry {
        at_ns: now,
        by,
        event,
    });
}

fn stored_action(id: ActionId) -> Result<StoredPending, WalletError> {
    storage::action(id).ok_or_else(|| WalletError::NotFound(format!("action {id}")))
}

/// Applies `change` to a stored action and stores the result, also when it
/// refuses: a refusal may have written down an expiry.
fn change_action(
    id: ActionId,
    change: impl FnOnce(&mut StoredPending, Principal, u64) -> Result<(), WalletError>,
) -> Result<Pending, WalletError> {
    let (caller, now) = (ic_cdk::api::msg_caller(), ic_cdk::api::time());
    let mut pending = stored_action(id)?;
    let result = change(&mut pending, caller, now);
    storage::set_action(id, pending.clone());
    result.map(|()| pending.view(id, now))
}

fn reason(error: &WalletError) -> String {
    match error {
        WalletError::Forbidden(reason)
        | WalletError::NotFound(reason)
        | WalletError::InvalidArgument(reason)
        | WalletError::PolicyViolation(reason) => reason.clone(),
        other => format!("{other:?}"),
    }
}

#[ic_cdk::query]
fn status() -> Status {
    let Some(config) = storage::config() else {
        ic_cdk::trap("the wallet has no configuration");
    };
    Status {
        version: env!("CARGO_PKG_VERSION").to_string(),
        claimed: storage::is_claimed(),
        claim_expires_at_ns: config.claim.map(|claim| claim.expires_at_ns),
        cycles: Nat::from(ic_cdk::api::canister_cycle_balance()),
        upgrade: storage::upgrade().view(ic_cdk::api::time()),
        release_channel: config.release_channel,
        bitcoin_network: storage::bitcoin_network(),
    }
}

/// The wallet's pages, certified. Anyone may call it, also before the claim.
#[ic_cdk::query]
fn http_request(request: ic_http_certification::HttpRequest<'static>) -> HttpResponse {
    let Some(certificate) = ic_cdk::api::data_certificate() else {
        ic_cdk::trap("http_request answers only as a query");
    };
    http::serve(&certificate, &request)
}

ic_cdk::export_candid!();

#[cfg(test)]
mod interface {
    use candid::TypeEnv;
    use candid::types::subtype::equal;
    use candid::types::{Type, TypeInner};
    use candid_parser::utils::CandidSource;
    use std::collections::HashSet;
    use std::path::Path;

    const TARGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/wallet.did");

    fn class(env: &TypeEnv, actor: &Type) -> (Vec<Type>, Vec<(String, Type)>) {
        let actor = env.trace_type(actor).expect("actor type");
        let TypeInner::Class(args, service) = actor.as_ref() else {
            panic!("the interface has no init arguments: {actor}");
        };
        let methods = env.as_service(service).expect("service type").to_vec();
        (args.clone(), methods)
    }

    /// Every method the canister exports must exist in `candid/wallet.did`
    /// with an equal type, mode included, and the init arguments must match.
    /// Methods not implemented yet are printed, not failed.
    #[test]
    fn implemented_methods_match_the_target_interface() {
        let exported = super::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let (target_env, target_actor) = CandidSource::File(Path::new(TARGET))
            .load()
            .expect("candid/wallet.did parses");
        let target_actor = env.merge_type(target_env, target_actor.expect("target service"));
        let (exported_args, exported_methods) =
            class(&env, &exported_actor.expect("exported service"));
        let (target_args, target_methods) = class(&env, &target_actor);

        let mut gamma = HashSet::new();
        assert_eq!(
            exported_args.len(),
            target_args.len(),
            "init argument count"
        );
        for (exported, target) in exported_args.iter().zip(&target_args) {
            equal(&mut gamma, &env, exported, target)
                .unwrap_or_else(|e| panic!("init argument {exported} differs from {target}: {e}"));
        }

        let mut mismatches = Vec::new();
        for (name, exported) in &exported_methods {
            match target_methods
                .iter()
                .find(|(target_name, _)| target_name == name)
            {
                None => mismatches.push(format!("{name}: not in candid/wallet.did")),
                Some((_, target)) => {
                    if let Err(e) = equal(&mut gamma, &env, exported, target) {
                        mismatches.push(format!("{name}: {exported} differs from {target}: {e}"));
                    }
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "interface mismatches:\n{}",
            mismatches.join("\n")
        );

        let missing: Vec<&str> = target_methods
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| {
                !exported_methods
                    .iter()
                    .any(|(exported, _)| exported == name)
            })
            .collect();
        eprintln!(
            "implemented {} of {} methods; not yet: {}",
            exported_methods.len(),
            target_methods.len(),
            missing.join(", ")
        );
    }
}
