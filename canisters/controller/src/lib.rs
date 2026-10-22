//! Recovery controller ([decision
//! 0009](../../../docs/decisions/0009-recovery-controller.md)): the only
//! controller of one wallet, with no controller itself.
//!
//! It changes the wallet's code, or hands the wallet to another controller,
//! only on an operation the owners' recovery keys signed, once the
//! operation's threshold of keys approved it and its delay passed. It reads
//! nothing from the wallet and gives a call from the wallet no more weight
//! than one from a stranger, which it refuses: only a recovery key's own
//! principal may call a method that changes state.

mod management;
mod operations;
mod storage;

use candid::{Nat, Principal};
use management::RunStatus;
use operations::{SentInstall, StoredOperation, StoredState, verify_all};
use recovery::controller::{
    ControllerArg, ControllerError, ControllerState, Execution, ExecutionStep, Health,
    OperationView, OperationsRequest, UploadChunk,
};
use recovery::history::{
    Change, ChangeDetails, ChangeOrigin, DeploymentMode, MAX_CHANGES, change_at,
    check_fresh_controller,
};
use recovery::{
    Context, DAY_NS, HOUR_NS, InstallCode, InstallMode, Operation, OperationKind, Signed,
};
use release::{CHUNK_SIZE, MAX_CHUNKS};
use serde_bytes::ByteBuf;
use std::cell::Cell;
use storage::{Config, StoredExecution};

pub const NOT_A_KEY: &str = "only a recovery key of this controller can do this";

/// The most operations open at once, and per proposing key.
const MAX_OPEN: usize = 16;
const MAX_OPEN_PER_KEY: usize = 4;
/// The most operations one key proposes in 24 hours. The controller keeps
/// every operation, open or closed, so this bounds how fast one key, stolen
/// or not, grows its storage: by at most 8 operations of at most about 72
/// KiB a day.
const MAX_PROPOSED_PER_KEY_PER_DAY: usize = 8;
/// The most chunks the controller keeps for all open operations together.
const MAX_STORED_CHUNKS: u64 = 2 * MAX_CHUNKS as u64;
/// The most operations one `operations` call returns.
const MAX_PAGE: usize = 10;

/// How long a step of an execution may take before the controller takes
/// the execution as dead. Every step is one call to the management
/// canister, which answers within minutes.
const STEP_TIMEOUT_NS: u64 = HOUR_NS;
/// How often the timer tries to start a wallet the controller left stopped.
const RESTART_RETRY_NS: u64 = 60 * 1_000_000_000;

/// The cycles the wallet must be able to spend, above its freezing reserve,
/// before the controller stops it to install: the IC holds back what the
/// install's instruction limit would cost and refuses the install when the
/// wallet cannot pay it. A 5.5 MB wallet release was refused at 329.9B on
/// icp-cli's local network, which charges as a 13-node subnet does.
const WALLET_INSTALL_CYCLES: u128 = 350_000_000_000;
/// The cycles the controller must be able to spend on an execution's calls,
/// and on each chunk it sends, whose every byte the caller pays for. The IC
/// holds back, for every call in flight, what the call's callback could
/// cost at its instruction limit, and refuses a message to a canister that
/// could not pay for its own execution: on PocketIC's 13-node subnet a
/// controller that could spend 25B could not take an `execute` message, and
/// one that could spend 60B could.
const OWN_CYCLES: u128 = 100_000_000_000;
const OWN_CYCLES_PER_CHUNK: u128 = 2_000_000_000;

#[ic_cdk::init]
fn init(arg: ControllerArg) {
    let ControllerArg::Init(init) = arg;
    let itself = ic_cdk::api::canister_self();
    if !recovery::is_canister_id(&init.wallet) || init.wallet == itself {
        ic_cdk::trap("the wallet must be another canister");
    }
    if let Err(reason) = init.policy.validate() {
        ic_cdk::trap(format!("invalid policy: {reason}"));
    }
    if let Err(reason) =
        recovery::check_proofs(&init.policy, &init.proofs, &[], itself, init.wallet)
    {
        ic_cdk::trap(format!("invalid proofs of possession: {reason}"));
    }
    storage::set_config(Config {
        wallet: init.wallet,
        policy: init.policy,
        policy_version: 1,
        next_nonce: 1,
        replaced_by: None,
        wallet_stopped: false,
    });
}

/// The controller has no controller, so nobody can upgrade it. A canister
/// that still has one cannot be upgraded to this module either, so a
/// canister that runs it holds only what its own `init` wrote, or a
/// snapshot, which `history::check_fresh_controller` looks for.
#[ic_cdk::post_upgrade]
fn post_upgrade() {
    ic_cdk::trap(
        "a recovery controller is never upgraded: install a new one and hand the wallet to it \
         with ReplaceController",
    );
}

/// Guard: the caller is a recovery key's self-authenticating principal.
/// ic-cdk runs it before it decodes the arguments, so a caller the
/// controller refuses never gets it to decode a chunk.
fn authority() -> Result<(), String> {
    if is_authority(ic_cdk::api::msg_caller()) {
        Ok(())
    } else {
        Err(NOT_A_KEY.to_string())
    }
}

fn is_authority(caller: Principal) -> bool {
    storage::config().policy.authority_of(&caller).is_some()
}

/// Drops every message from outside the IC that is not from a recovery
/// key before it reaches the canister. The controller pays for every
/// message it takes, a query sent as an update among them, so this keeps
/// strangers from spending its cycles for free. Query calls, which run on
/// one node and cost nothing, skip this, so anyone can still read the
/// controller; so do calls from canisters, which pay for their own calls,
/// and which the guard refuses where a method changes state.
#[ic_cdk::inspect_message]
fn inspect_message() {
    if is_authority(ic_cdk::api::msg_caller()) {
        ic_cdk::api::accept_message();
    }
}

#[ic_cdk::query]
fn state() -> ControllerState {
    let config = storage::config();
    ControllerState {
        wallet: config.wallet,
        policy_sha256: ByteBuf::from(config.policy.sha256().to_vec()),
        policy: config.policy,
        policy_version: config.policy_version,
        next_nonce: config.next_nonce,
        replaced_by: config.replaced_by,
        execution: storage::execution().map(|execution| Execution {
            operation: execution.operation,
            step: execution.step,
            since_ns: execution.since_ns,
        }),
        wallet_stopped: config.wallet_stopped,
        cycles: Nat::from(ic_cdk::api::canister_cycle_balance()),
    }
}

#[ic_cdk::query]
fn operation(id: u64) -> Option<OperationView> {
    let (config, now) = (storage::config(), ic_cdk::api::time());
    storage::operation(id).map(|stored| view(id, &stored, &config, now))
}

#[ic_cdk::query]
fn operations(request: OperationsRequest) -> Vec<OperationView> {
    let (config, now) = (storage::config(), ic_cdk::api::time());
    let limit = usize::from(request.limit).min(MAX_PAGE);
    storage::operations_page(request.before, limit)
        .into_iter()
        .map(|(id, stored)| view(id, &stored, &config, now))
        .collect()
}

/// What the IC says about this controller's settings, and the subnet it
/// runs on. A wallet reads it before it hands itself to this controller,
/// and so does a controller before it hands its wallet over.
#[ic_cdk::update]
async fn health() -> Health {
    management::own_health()
        .await
        .unwrap_or_else(|reason| ic_cdk::trap(reason))
}

/// Proposes an operation, with the approvals in `signed`, which may be
/// none. Its nonce must be the controller's next one; it becomes its id.
#[ic_cdk::update(guard = "authority")]
fn propose(operation: Operation, signed: Vec<Signed>) -> Result<OperationView, ControllerError> {
    let now = ic_cdk::api::time();
    let mut config = active()?;
    let proposer = caller_key(&config);
    settle_open(&config, now);
    let open = storage::open_operations();
    if open.len() >= MAX_OPEN {
        return Err(ControllerError::Forbidden(format!(
            "{MAX_OPEN} operations are open; cancel one or let one expire"
        )));
    }
    if open
        .iter()
        .filter(|(_, stored)| stored.proposed_by == proposer)
        .count()
        >= MAX_OPEN_PER_KEY
    {
        return Err(ControllerError::Forbidden(format!(
            "this key proposed {MAX_OPEN_PER_KEY} operations that are open"
        )));
    }
    if storage::proposed_since(now.saturating_sub(DAY_NS))
        .iter()
        .filter(|stored| stored.proposed_by == proposer)
        .count()
        >= MAX_PROPOSED_PER_KEY_PER_DAY
    {
        return Err(ControllerError::Forbidden(format!(
            "this key proposed {MAX_PROPOSED_PER_KEY_PER_DAY} operations in the last 24 hours"
        )));
    }
    let route = recovery::check_proposal(
        &operation,
        &Context {
            controller: ic_cdk::api::canister_self(),
            wallet: config.wallet,
            policy: &config.policy,
            policy_version: config.policy_version,
            next_nonce: config.next_nonce,
            now,
        },
    )
    .map_err(ControllerError::InvalidArgument)?;
    let approvals = if signed.is_empty() {
        Vec::new()
    } else {
        let message = recovery::approve_message(&operation.digest());
        verify_all(&config.policy, &message, &signed)?
    };
    let id = operation.nonce;
    let requirement = config.policy.requirement(route);
    let stored = StoredOperation::new(operation, route, requirement, approvals, proposer, now);
    config.next_nonce += 1;
    storage::set_config(config.clone());
    storage::set_operation(id, stored.clone());
    Ok(view(id, &stored, &config, now))
}

/// Adds approvals: signatures over the operation's approve message, each by
/// a key that has not approved it yet.
#[ic_cdk::update(guard = "authority")]
fn approve(id: u64, signed: Vec<Signed>) -> Result<OperationView, ControllerError> {
    let now = ic_cdk::api::time();
    let config = active()?;
    not_running(id, now)?;
    let mut stored = stored_operation(id)?;
    let result = stored
        .check_open(config.policy_version, now)
        .and_then(|()| {
            verify_all(
                &config.policy,
                &recovery::approve_message(&stored.digest()),
                &signed,
            )
        })
        .and_then(|keys| stored.approve(keys, config.policy_version, now));
    storage::set_operation(id, stored.clone());
    result.map(|()| view(id, &stored, &config, now))
}

/// Cancels an open operation on a signature over its cancel message by any
/// key, except a `SetPolicy` that removes that key.
#[ic_cdk::update(guard = "authority")]
fn cancel(id: u64, signed: Signed) -> Result<OperationView, ControllerError> {
    let now = ic_cdk::api::time();
    let config = active()?;
    not_running(id, now)?;
    let mut stored = stored_operation(id)?;
    let result = stored
        .check_open(config.policy_version, now)
        .and_then(|()| {
            verify_all(
                &config.policy,
                &recovery::cancel_message(&stored.digest()),
                std::slice::from_ref(&signed),
            )
        })
        .and_then(|_| stored.cancel(signed.public_key, config.policy_version, now));
    storage::set_operation(id, stored.clone());
    result.map(|()| view(id, &stored, &config, now))
}

/// Stores a chunk of an open `InstallCode` whose threshold is met, if it
/// hashes to the approved chunk hash at its index, and returns the chunks
/// still missing. Only an approved operation takes chunks, so a key alone
/// cannot make the controller pay to store modules nobody approved.
#[ic_cdk::update(guard = "authority")]
fn upload_chunk(args: UploadChunk) -> Result<Vec<u32>, ControllerError> {
    let now = ic_cdk::api::time();
    let config = active()?;
    not_running(args.operation, now)?;
    let id = args.operation;
    let mut stored = stored_operation(id)?;
    let open = stored.check_open(config.policy_version, now);
    storage::set_operation(id, stored.clone());
    open?;
    let install = stored.install().ok_or_else(|| {
        ControllerError::InvalidArgument(format!("operation {id} installs no code"))
    })?;
    if stored.approved_at_ns.is_none() {
        return Err(ControllerError::Forbidden(format!(
            "operation {id} takes chunks once {} keys approved it",
            stored.threshold
        )));
    }
    let hash = install
        .chunk_hashes
        .get(args.index as usize)
        .ok_or_else(|| {
            ControllerError::InvalidArgument(format!(
                "operation {id} has {} chunks",
                install.chunk_hashes.len()
            ))
        })?;
    if args.chunk.is_empty() || args.chunk.len() > CHUNK_SIZE {
        return Err(ControllerError::InvalidArgument(format!(
            "a chunk has 1 to {CHUNK_SIZE} bytes"
        )));
    }
    if recovery::sha256(&args.chunk).as_slice() != hash.as_slice() {
        return Err(ControllerError::InvalidArgument(format!(
            "chunk {} does not hash to the approved chunk hash",
            args.index
        )));
    }
    if !storage::has_chunk(id, args.index) && storage::chunk_count() >= MAX_STORED_CHUNKS {
        return Err(ControllerError::Forbidden(format!(
            "the controller holds {MAX_STORED_CHUNKS} chunks; execute or cancel an operation \
             first"
        )));
    }
    storage::set_chunk(id, args.index, args.chunk.into_vec());
    Ok(missing_chunks(id, install))
}

/// Runs an operation whose threshold is met and whose delay passed. A
/// failed execution leaves it open, and the wallet running.
#[ic_cdk::update(guard = "authority")]
async fn execute(id: u64) -> Result<OperationView, ControllerError> {
    let now = ic_cdk::api::time();
    let config = active()?;
    check_no_execution(now)?;
    settle_open(&config, now);
    let mut stored = stored_operation(id)?;
    let ready = stored.check_ready(config.policy_version, now);
    storage::set_operation(id, stored.clone());
    ready?;
    match stored.operation.kind.clone() {
        OperationKind::SetPolicy { policy, .. } => {
            let mut config = config;
            config.policy = policy;
            config.policy_version += 1;
            storage::set_config(config.clone());
            stored.attempts += 1;
            stored.executed(now);
            storage::set_operation(id, stored);
            // Every other open operation was approved under the old policy.
            settle_open(&config, now);
            view_of(id)
        }
        OperationKind::InstallCode(install) => {
            let missing = missing_chunks(id, &install);
            if !missing.is_empty() {
                return Err(ControllerError::ChunksMissing(missing));
            }
            check_own_cycles(own_cycles(&install))?;
            let running = begin_attempt(id);
            let result = install_code(id, &install, &running).await;
            finish(id, running, result).await
        }
        OperationKind::ReplaceController {
            controller,
            module_hash,
            policy_sha256,
        } => {
            check_own_cycles(OWN_CYCLES)?;
            let running = begin_attempt(id);
            let result =
                replace_controller(id, controller, &module_hash, &policy_sha256, &running).await;
            finish(id, running, result).await
        }
    }
}

/// Starts the wallet again, for instance when an execution stopped it and
/// starting it failed. The timer does the same every minute while the
/// controller knows it left the wallet stopped.
#[ic_cdk::update(guard = "authority")]
async fn start_wallet() -> Result<(), ControllerError> {
    let now = ic_cdk::api::time();
    let config = active()?;
    check_no_execution(now)?;
    let _running = Running::begin(None, ExecutionStep::Starting);
    management::start(config.wallet).await.map_err(failed)?;
    set_wallet_stopped(false);
    Ok(())
}

/// Installs the approved module in the wallet. An upgrade stops the wallet
/// first, so no callback of the old code runs in the new. A reinstall
/// uninstalls the old code instead, which drops its callbacks, so code that
/// keeps a call in flight, and so never lets the wallet stop, cannot hold up
/// a reinstall. The controller then gives the wallet the cycles it lacks for
/// the install, stages the module in the wallet's own chunk store, and
/// installs it. The caller starts the wallet again.
///
/// The wallet's code can fill or empty its chunk store, or spend its
/// cycles, only while it runs, so the controller reads the wallet's cycles
/// and stages the module only once it runs no code. The store is the
/// wallet's own, so the controller may live on another subnet.
async fn install_code(
    id: u64,
    install: &InstallCode,
    running: &Running,
) -> Result<(), ControllerError> {
    let wallet = storage::config().wallet;
    let status = management::wallet_status(wallet).await.map_err(|reason| {
        failed(format!(
            "cannot read the wallet's status, which only a controller of the wallet can: \
             {reason}"
        ))
    })?;
    running.check()?;
    if let Some(sent) = stored_operation(id)?.install_sent {
        if install_ran(wallet, install, sent, running).await? {
            mark_executed(id);
            return Ok(());
        }
        set_install_sent(id, None);
    }
    if let Some(manifest) = &install.release {
        let tested = status.module_hash.as_ref().is_some_and(|current| {
            manifest
                .tested_from
                .iter()
                .any(|from| from.as_slice() == current.as_slice())
        });
        if !tested {
            return Err(ControllerError::Forbidden(format!(
                "release {} was not tested to upgrade from the module the wallet runs; install \
                 it on the sovereign route",
                manifest.version
            )));
        }
    }
    // The controller pays what the wallet lacks: the wallet's code can spend
    // the wallet's cycles, never the controller's. It refuses now, before
    // it stops anything, if it could not pay what the wallet lacks now.
    let own = own_cycles(install);
    check_own_cycles(own + WALLET_INSTALL_CYCLES.saturating_sub(status.liquid_cycles))?;
    set_wallet_stopped(true);
    match install.mode {
        InstallMode::Reinstall => {
            running.step(ExecutionStep::Uninstalling)?;
            management::uninstall(wallet).await.map_err(failed)?;
        }
        InstallMode::Upgrade { .. } => {
            // Stopping waits until every call the wallet made is answered.
            if status.status != RunStatus::Stopped {
                running.step(ExecutionStep::Stopping)?;
                management::stop(wallet).await.map_err(|reason| {
                    failed(format!(
                        "the wallet did not stop: {reason}. Code that keeps a call in flight \
                         never lets it stop; a Reinstall uninstalls such code instead"
                    ))
                })?;
            }
            running.step(ExecutionStep::Staging)?;
            management::clear_chunk_store(wallet)
                .await
                .map_err(failed)?;
        }
    }
    // The wallet runs no code now, so only its idle burn lowers its cycles.
    running.step(ExecutionStep::ToppingUp)?;
    let liquid = management::wallet_status(wallet)
        .await
        .map_err(failed)?
        .liquid_cycles;
    let lacking = WALLET_INSTALL_CYCLES.saturating_sub(liquid);
    if lacking > 0 {
        running.step(ExecutionStep::ToppingUp)?;
        check_own_cycles(own + lacking)?;
        management::deposit_cycles(wallet, lacking)
            .await
            .map_err(failed)?;
    }
    for (index, hash) in install.chunk_hashes.iter().enumerate() {
        running.step(ExecutionStep::Staging)?;
        let index = index as u32;
        let chunk =
            storage::chunk(id, index).ok_or_else(|| ControllerError::ChunksMissing(vec![index]))?;
        let stored_as = management::upload_chunk(wallet, chunk)
            .await
            .map_err(failed)?;
        if stored_as != hash.as_slice() {
            return Err(failed(format!(
                "the IC stored chunk {index} under another hash"
            )));
        }
    }
    running.step(ExecutionStep::Installing)?;
    let before = management::info(wallet, 0)
        .await
        .map_err(failed)?
        .total_num_changes;
    running.step(ExecutionStep::Installing)?;
    // No await until the install is sent, so it goes with this version.
    set_install_sent(
        id,
        Some(SentInstall {
            changes_before: before,
            sender_version: ic_cdk::api::canister_version(),
        }),
    );
    if let Err(reason) = management::install(
        wallet,
        install.mode,
        install.chunk_hashes.clone(),
        install.wasm_sha256.clone(),
        install.arg.clone(),
    )
    .await
    {
        // A reject means the install changed nothing.
        set_install_sent(id, None);
        return Err(failed(reason));
    }
    mark_executed(id);
    Ok(())
}

/// Whether the install an earlier execution sent ran: the change that
/// followed in the wallet's history is that install, made by this
/// controller at the version it sent it with.
async fn install_ran(
    wallet: Principal,
    install: &InstallCode,
    sent: SentInstall,
    running: &Running,
) -> Result<bool, ControllerError> {
    let info = management::info(wallet, MAX_CHANGES)
        .await
        .map_err(failed)?;
    running.check()?;
    if info.total_num_changes <= sent.changes_before {
        return Ok(false);
    }
    let installed = Change {
        origin: ChangeOrigin::FromCanister {
            canister_id: ic_cdk::api::canister_self(),
            canister_version: Some(sent.sender_version),
        },
        details: ChangeDetails::CodeDeployment {
            mode: match install.mode {
                InstallMode::Upgrade { .. } => DeploymentMode::Upgrade,
                InstallMode::Reinstall => DeploymentMode::Install,
            },
            module_hash: install.wasm_sha256.clone(),
        },
    };
    match change_at(&info, sent.changes_before) {
        Some(change) => Ok(*change == installed),
        None => Err(failed(
            "the wallet changed too often since an earlier execution sent this install to tell \
             whether it ran; cancel the operation and propose it again"
                .to_string(),
        )),
    }
}

fn set_install_sent(id: u64, sent: Option<SentInstall>) {
    if let Some(mut stored) = storage::operation(id) {
        stored.install_sent = sent;
        storage::set_operation(id, stored);
    }
}

fn set_hand_over_sent(id: u64, sent: bool) {
    if let Some(mut stored) = storage::operation(id) {
        stored.hand_over_sent = sent;
        storage::set_operation(id, stored);
    }
}

/// The cycles the controller needs for an install's calls and chunks.
fn own_cycles(install: &InstallCode) -> u128 {
    OWN_CYCLES + OWN_CYCLES_PER_CHUNK * install.chunk_hashes.len() as u128
}

/// Hands the wallet to another controller once the IC shows that it runs
/// the approved module, was installed fresh and has no controller, and it
/// says it serves this wallet with the approved policy, has run nothing yet,
/// runs on this controller's subnet and has settings that cannot strand the
/// wallet.
async fn replace_controller(
    id: u64,
    controller: Principal,
    module_hash: &[u8],
    policy_sha256: &[u8],
    running: &Running,
) -> Result<(), ControllerError> {
    let config = storage::config();
    let itself = ic_cdk::api::canister_self();
    let wallet = management::info(config.wallet, 0).await.map_err(failed)?;
    running.check()?;
    // An earlier execution sent the hand-over and stopped before it learned
    // the answer.
    if stored_operation(id)?.hand_over_sent {
        if wallet.controllers == [controller] {
            hand_over_done(id, controller);
            return Ok(());
        }
        set_hand_over_sent(id, false);
    }
    if !wallet.controllers.contains(&itself) {
        return Err(failed(
            "this controller does not control the wallet".to_string(),
        ));
    }
    if config.wallet_stopped {
        return Err(ControllerError::Forbidden(
            "the wallet is stopped; start it with start_wallet before handing it over".to_string(),
        ));
    }
    let info = management::info(controller, MAX_CHANGES)
        .await
        .map_err(failed)?;
    running.check()?;
    check_fresh_controller(&info, module_hash)
        .map_err(|reason| failed(format!("controller {controller}: {reason}")))?;
    let other = management::other_state(controller).await.map_err(failed)?;
    running.check()?;
    if other.wallet != config.wallet {
        return Err(failed(format!(
            "controller {controller} serves wallet {}, not this one",
            other.wallet
        )));
    }
    if other.policy_sha256.as_slice() != policy_sha256 {
        return Err(failed(format!(
            "controller {controller} holds another policy than the approved one"
        )));
    }
    if let Some(next) = other.replaced_by {
        return Err(failed(format!(
            "controller {controller} handed its wallet to {next} already"
        )));
    }
    if other.policy_version != 1 || other.next_nonce != 1 {
        return Err(failed(format!(
            "controller {controller} has changed its policy or taken operations already; hand \
             the wallet only to a controller as its install left it"
        )));
    }
    let health = management::other_health(controller).await.map_err(failed)?;
    running.check()?;
    health
        .check(ic_cdk::api::subnet_self())
        .map_err(|reason| failed(format!("controller {controller}: {reason}")))?;
    running.step(ExecutionStep::HandingOver)?;
    set_hand_over_sent(id, true);
    if let Err(reason) = management::hand_over(config.wallet, controller).await {
        // A reject means the controllers did not change.
        set_hand_over_sent(id, false);
        return Err(failed(reason));
    }
    hand_over_done(id, controller);
    Ok(())
}

/// Records that the wallet belongs to `controller` now: this controller
/// acts no more, and every other open operation goes stale.
fn hand_over_done(id: u64, controller: Principal) {
    let mut config = storage::config();
    config.replaced_by = Some(controller);
    storage::set_config(config);
    mark_executed(id);
    for (other, mut stored) in storage::open_operations() {
        if other != id {
            stored.state = StoredState::Stale;
            storage::set_operation(other, stored);
        }
    }
}

/// Empties the wallet's chunk store and starts the wallet again if the
/// execution left it stopped, and records how the execution ended.
async fn finish(
    id: u64,
    running: Running,
    result: Result<(), ControllerError>,
) -> Result<OperationView, ControllerError> {
    let config = storage::config();
    let started = if config.wallet_stopped && running.owns() {
        // The staged module would cost the wallet storage. Leaving it is
        // only a waste: the next execution empties the store first.
        let _ = management::clear_chunk_store(config.wallet).await;
        start_stopped_wallet(&running).await
    } else {
        Ok(())
    };
    drop(running);
    let mut stored = stored_operation(id)?;
    let note = match (&result, started) {
        (Err(error), _) => Some(describe(error)),
        (Ok(()), Err(reason)) => Some(format!(
            "executed, but starting the wallet again failed: {reason}; the controller tries \
             again every minute"
        )),
        (Ok(()), Ok(())) => None,
    };
    if note.is_some() {
        stored.last_error = note;
        storage::set_operation(id, stored);
    }
    result.and_then(|()| view_of(id))
}

async fn start_stopped_wallet(running: &Running) -> Result<(), String> {
    running
        .step(ExecutionStep::Starting)
        .map_err(|e| describe(&e))?;
    management::start(storage::config().wallet).await?;
    set_wallet_stopped(false);
    Ok(())
}

fn begin_attempt(id: u64) -> Running {
    let running = Running::begin(Some(id), ExecutionStep::Checking);
    if let Some(mut stored) = storage::operation(id) {
        stored.attempts += 1;
        storage::set_operation(id, stored);
    }
    running
}

fn mark_executed(id: u64) {
    if let Some(mut stored) = storage::operation(id) {
        stored.executed(ic_cdk::api::time());
        storage::set_operation(id, stored);
    }
}

fn set_wallet_stopped(stopped: bool) {
    let mut config = storage::config();
    config.wallet_stopped = stopped;
    storage::set_config(config);
}

/// The execution lock, held while an execution or a restart of the wallet
/// runs. It is given up when dropped: when the execution returns, and when
/// code after one of its calls traps, since ic-cdk then drops the call's
/// future and keeps what this writes. A step that never ends is taken as
/// dead after `STEP_TIMEOUT_NS`; an execution that comes back after that
/// finds it no longer holds the lock and goes no further.
struct Running {
    operation: Option<u64>,
    since_ns: Cell<u64>,
}

impl Running {
    fn begin(operation: Option<u64>, step: ExecutionStep) -> Running {
        let running = Running {
            operation,
            since_ns: Cell::new(0),
        };
        running.write(step);
        running
    }

    fn write(&self, step: ExecutionStep) {
        let now = ic_cdk::api::time();
        self.since_ns.set(now);
        storage::set_execution(Some(StoredExecution {
            operation: self.operation,
            step,
            since_ns: now,
        }));
        // Starts the wallet again if this step never ends.
        ic_cdk::api::global_timer_set(now.saturating_add(STEP_TIMEOUT_NS));
    }

    fn owns(&self) -> bool {
        storage::execution().is_some_and(|execution| {
            execution.operation == self.operation && execution.since_ns == self.since_ns.get()
        })
    }

    /// Refuses to go on once the lock was taken from this execution.
    fn check(&self) -> Result<(), ControllerError> {
        if self.owns() {
            Ok(())
        } else {
            Err(failed(
                "a step took longer than an hour, so the controller took this execution as \
                 dead"
                    .to_string(),
            ))
        }
    }

    /// Records the step about to run.
    fn step(&self, step: ExecutionStep) -> Result<(), ControllerError> {
        self.check()?;
        self.write(step);
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.owns() {
            storage::set_execution(None);
            schedule_restart(ic_cdk::api::time());
        }
    }
}

/// Sets the timer to start the wallet again if the controller left it
/// stopped, or turns it off.
fn schedule_restart(now: u64) {
    let config = storage::config();
    let at = if config.wallet_stopped && config.replaced_by.is_none() {
        now.saturating_add(RESTART_RETRY_NS)
    } else {
        0
    };
    ic_cdk::api::global_timer_set(at);
}

/// Takes a step that never ended as dead, and starts a wallet the
/// controller left stopped.
fn on_timer(now: u64) {
    if let Some(execution) = storage::execution() {
        let dead_at = execution.since_ns.saturating_add(STEP_TIMEOUT_NS);
        if now < dead_at {
            ic_cdk::api::global_timer_set(dead_at);
            return;
        }
        storage::set_execution(None);
        if let Some(id) = execution.operation
            && let Some(mut stored) = storage::operation(id)
            && stored.is_open()
        {
            stored.last_error = Some(format!(
                "the execution stopped at step {:?} and did not go on",
                execution.step
            ));
            storage::set_operation(id, stored);
        }
    }
    let config = storage::config();
    if config.wallet_stopped && config.replaced_by.is_none() {
        ic_cdk::futures::spawn(async move {
            let _running = Running::begin(None, ExecutionStep::Starting);
            if management::start(config.wallet).await.is_ok() {
                set_wallet_stopped(false);
            }
        });
    }
}

#[cfg_attr(target_family = "wasm", unsafe(export_name = "canister_global_timer"))]
#[cfg_attr(
    not(target_family = "wasm"),
    unsafe(export_name = "canister_global_timer_host")
)]
fn global_timer() {
    ic_cdk::futures::internals::in_executor_context(|| on_timer(ic_cdk::api::time()));
}

/// The configuration, unless the wallet was handed to another controller.
fn active() -> Result<Config, ControllerError> {
    let config = storage::config();
    match config.replaced_by {
        Some(next) => Err(ControllerError::Replaced(next)),
        None => Ok(config),
    }
}

/// The caller's key; the guard checked there is one.
fn caller_key(config: &Config) -> ByteBuf {
    config
        .policy
        .authority_of(&ic_cdk::api::msg_caller())
        .map(|authority| authority.public_key.clone())
        .unwrap_or_else(|| ic_cdk::trap(NOT_A_KEY))
}

/// The execution in flight, unless its step outlived `STEP_TIMEOUT_NS`.
fn live_execution(now: u64) -> Option<StoredExecution> {
    storage::execution()
        .filter(|execution| now < execution.since_ns.saturating_add(STEP_TIMEOUT_NS))
}

fn check_no_execution(now: u64) -> Result<(), ControllerError> {
    match live_execution(now) {
        Some(execution) => Err(ControllerError::ExecutionInProgress {
            operation: execution.operation,
        }),
        None => {
            // A dead step's lock is dropped here, so the next one can go.
            if storage::execution().is_some() {
                storage::set_execution(None);
            }
            Ok(())
        }
    }
}

/// Refuses to change the operation an execution runs.
fn not_running(id: u64, now: u64) -> Result<(), ControllerError> {
    match live_execution(now) {
        Some(execution) if execution.operation == Some(id) => {
            Err(ControllerError::ExecutionInProgress {
                operation: Some(id),
            })
        }
        _ => Ok(()),
    }
}

/// Ends the open operations whose expiry passed or whose policy version is
/// old, which deletes their chunks. The operation an execution runs is left
/// alone: it was ready when the execution began.
fn settle_open(config: &Config, now: u64) {
    let running = live_execution(now).and_then(|execution| execution.operation);
    for (id, mut stored) in storage::open_operations() {
        if Some(id) != running && stored.settle(config.policy_version, now) {
            storage::set_operation(id, stored);
        }
    }
}

fn check_own_cycles(needed: u128) -> Result<(), ControllerError> {
    let available = ic_cdk::api::canister_liquid_cycle_balance();
    if available < needed {
        return Err(ControllerError::InsufficientCycles {
            canister: ic_cdk::api::canister_self(),
            available: Nat::from(available),
            needed: Nat::from(needed),
        });
    }
    Ok(())
}

fn stored_operation(id: u64) -> Result<StoredOperation, ControllerError> {
    storage::operation(id).ok_or_else(|| ControllerError::NotFound(format!("operation {id}")))
}

fn missing_chunks(id: u64, install: &InstallCode) -> Vec<u32> {
    (0..install.chunk_hashes.len() as u32)
        .filter(|&index| !storage::has_chunk(id, index))
        .collect()
}

fn view(id: u64, stored: &StoredOperation, config: &Config, now: u64) -> OperationView {
    let missing_chunks = match stored.install() {
        Some(install) if stored.is_open() => missing_chunks(id, install),
        _ => Vec::new(),
    };
    OperationView {
        id,
        operation: stored.operation.clone(),
        digest: stored.digest.clone(),
        route: stored.route,
        threshold: stored.threshold,
        delay_ns: stored.delay_ns,
        approvals: stored.approvals.clone(),
        proposed_by: stored.proposed_by.clone(),
        proposed_at_ns: stored.proposed_at_ns,
        approved_at_ns: stored.approved_at_ns,
        missing_chunks,
        status: stored.status(config.policy_version, now),
        attempts: stored.attempts,
        last_error: stored.last_error.clone(),
    }
}

fn view_of(id: u64) -> Result<OperationView, ControllerError> {
    let stored = stored_operation(id)?;
    Ok(view(id, &stored, &storage::config(), ic_cdk::api::time()))
}

fn failed(reason: String) -> ControllerError {
    ControllerError::ExecutionFailed(reason)
}

fn describe(error: &ControllerError) -> String {
    match error {
        ControllerError::Forbidden(reason)
        | ControllerError::InvalidArgument(reason)
        | ControllerError::NotFound(reason)
        | ControllerError::BadSignature(reason)
        | ControllerError::ExecutionFailed(reason) => reason.clone(),
        other => format!("{other:?}"),
    }
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

    const TARGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../candid/controller.did");

    fn class(env: &TypeEnv, actor: &Type) -> (Vec<Type>, Vec<(String, Type)>) {
        let actor = env.trace_type(actor).expect("actor type");
        let TypeInner::Class(args, service) = actor.as_ref() else {
            panic!("the interface has no init arguments: {actor}");
        };
        let methods = env.as_service(service).expect("service type").to_vec();
        (args.clone(), methods)
    }

    /// The canister exports exactly the methods of `candid/controller.did`,
    /// each with an equal type, mode included, and the same init argument.
    #[test]
    fn exported_interface_equals_the_target() {
        let exported = super::__export_service();
        let (mut env, exported_actor) = CandidSource::Text(&exported)
            .load()
            .expect("exported .did parses");
        let (target_env, target_actor) = CandidSource::File(Path::new(TARGET))
            .load()
            .expect("candid/controller.did parses");
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
        let mut exported_names: Vec<&str> = exported_methods
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let mut target_names: Vec<&str> = target_methods
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        exported_names.sort_unstable();
        target_names.sort_unstable();
        assert_eq!(exported_names, target_names);
        for (name, exported) in &exported_methods {
            let (_, target) = target_methods
                .iter()
                .find(|(target_name, _)| target_name == name)
                .expect("listed above");
            equal(&mut gamma, &env, exported, target)
                .unwrap_or_else(|e| panic!("{name}: {exported} differs from {target}: {e}"));
        }
    }
}
