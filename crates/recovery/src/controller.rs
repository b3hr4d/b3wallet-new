//! The recovery controller's interface types (`candid/controller.did`),
//! shared by the controller and the clients that call it.

use crate::{Operation, Policy, Proof, Route};
use candid::{CandidType, Nat, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// A controller is installed once and never upgraded: it has no controller.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ControllerArg {
    Init(ControllerInit),
}

/// The wallet a controller serves, and its first policy with a proof of
/// possession for every key.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ControllerInit {
    pub wallet: Principal,
    pub policy: Policy,
    pub proofs: Vec<Proof>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ControllerState {
    pub wallet: Principal,
    pub policy: Policy,
    pub policy_version: u64,
    pub policy_sha256: ByteBuf,
    /// The nonce the next operation must carry.
    pub next_nonce: u64,
    /// The controller the wallet was handed to; this one then acts no more.
    pub replaced_by: Option<Principal>,
    pub execution: Option<Execution>,
    /// Whether the controller stopped the wallet and has not started it
    /// again yet.
    pub wallet_stopped: bool,
    pub cycles: Nat,
}

/// The execution in flight.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Execution {
    /// `None` while the controller only starts the wallet again.
    pub operation: Option<u64>,
    pub step: ExecutionStep,
    /// When the step began.
    pub since_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionStep {
    Checking,
    Stopping,
    /// A reinstall uninstalls the wallet's code instead of stopping it.
    Uninstalling,
    /// The controller gives the wallet the cycles it lacks for the install.
    ToppingUp,
    Staging,
    Installing,
    Starting,
    HandingOver,
}

/// What a controller's `health` method says about itself: its subnet, and
/// the settings and cycles the IC's `canister_status` gives. Nobody can
/// change a controller's settings once it has no controller, so settings
/// its creator chose could strand the wallet it is handed.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub subnet: Principal,
    pub controllers: Vec<Principal>,
    /// Seconds of idle burn the controller keeps in reserve; below it, the
    /// controller is frozen until someone tops it up.
    pub freezing_threshold: Nat,
    pub reserved_cycles_limit: Nat,
    pub wasm_memory_limit: Nat,
    pub memory_allocation: Nat,
    pub compute_allocation: Nat,
    pub cycles: Nat,
    pub idle_cycles_burned_per_day: Nat,
}

/// The IC's defaults: a freezing threshold of 30 days, a reserved cycles
/// limit of 5T cycles and a Wasm memory limit of 3 GiB.
pub const MIN_FREEZING_THRESHOLD_S: u64 = 30 * 86_400;
/// A larger threshold keeps the controller frozen behind a reserve its
/// owners would have to pay; an unbounded one, forever.
pub const MAX_FREEZING_THRESHOLD_S: u64 = 365 * 86_400;
/// With a lower limit the controller cannot grow its memory while its
/// subnet is nearly full, so it could not store an operation.
pub const MIN_RESERVED_CYCLES_LIMIT: u128 = 5_000_000_000_000;
/// With a lower limit (0 is none) its heap could not grow.
pub const MIN_WASM_MEMORY_LIMIT: u64 = 1 << 30;

impl Health {
    /// Checks that the controller is on `subnet`, the wallet's, so it adds
    /// no subnet the wallet depends on, and that its settings cannot strand
    /// the wallet: no controller, a freezing threshold of 30 to 365 days,
    /// no allocations to pay for, and the default limits or higher.
    pub fn check(&self, subnet: Principal) -> Result<(), String> {
        if self.subnet != subnet {
            return Err(format!(
                "it runs on subnet {}, not the wallet's subnet {subnet}",
                self.subnet
            ));
        }
        if !self.controllers.is_empty() {
            return Err("it has controllers; it must have none".to_string());
        }
        let at_most = |value: &Nat, limit: u128| u128::try_from(&value.0).is_ok_and(|v| v <= limit);
        let at_least =
            |value: &Nat, limit: u128| u128::try_from(&value.0).map_or(true, |v| v >= limit);
        if !at_least(&self.freezing_threshold, MIN_FREEZING_THRESHOLD_S.into())
            || !at_most(&self.freezing_threshold, MAX_FREEZING_THRESHOLD_S.into())
        {
            return Err(format!(
                "its freezing threshold is {} seconds, not 30 to 365 days",
                self.freezing_threshold
            ));
        }
        if !at_most(&self.memory_allocation, 0) || !at_most(&self.compute_allocation, 0) {
            return Err(
                "it has a memory or compute allocation, which burns its cycles".to_string(),
            );
        }
        if !at_least(&self.reserved_cycles_limit, MIN_RESERVED_CYCLES_LIMIT) {
            return Err(format!(
                "its reserved cycles limit is {}, below the IC's default of 5T",
                self.reserved_cycles_limit
            ));
        }
        if !at_most(&self.wasm_memory_limit, 0)
            && !at_least(&self.wasm_memory_limit, MIN_WASM_MEMORY_LIMIT.into())
        {
            return Err(format!(
                "its Wasm memory limit is {} bytes, below 1 GiB",
                self.wasm_memory_limit
            ));
        }
        Ok(())
    }
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OperationView {
    pub id: u64,
    pub operation: Operation,
    pub digest: ByteBuf,
    pub route: Route,
    pub threshold: u8,
    pub delay_ns: u64,
    /// The keys whose approvals count, in the order they came.
    pub approvals: Vec<ByteBuf>,
    pub proposed_by: ByteBuf,
    pub proposed_at_ns: u64,
    /// When the threshold was met and the delay started.
    pub approved_at_ns: Option<u64>,
    /// The chunks of an `InstallCode` the controller does not hold yet.
    pub missing_chunks: Vec<u32>,
    pub status: OperationStatus,
    /// How many executions started; a failed one leaves the operation open.
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum OperationStatus {
    /// Waiting for its threshold of approvals.
    Approving,
    /// Approved, waiting for its delay to end.
    Waiting {
        ready_at_ns: u64,
    },
    /// Can be executed now.
    Ready,
    Executed {
        at_ns: u64,
    },
    Cancelled {
        by: ByteBuf,
        at_ns: u64,
    },
    /// Past its expiry before it ran.
    Expired,
    /// The policy changed, or the controller handed the wallet over, before
    /// it ran.
    Stale,
}

impl OperationStatus {
    pub fn is_open(&self) -> bool {
        matches!(
            self,
            OperationStatus::Approving | OperationStatus::Waiting { .. } | OperationStatus::Ready
        )
    }
}

/// Newest first: up to `limit`, at most 10, before `before`, or from the
/// newest.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OperationsRequest {
    pub before: Option<u64>,
    pub limit: u8,
}

/// One chunk of an `InstallCode`'s module.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct UploadChunk {
    pub operation: u64,
    pub index: u32,
    pub chunk: ByteBuf,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ControllerError {
    /// The caller is not one of the policy's recovery keys, or the call is
    /// not allowed now; the text says why.
    Forbidden(String),
    InvalidArgument(String),
    NotFound(String),
    /// A signature that does not verify, or a key the policy does not hold.
    BadSignature(String),
    /// The operation ran, was cancelled, expired or went stale.
    Closed(OperationStatus),
    QuorumNotMet {
        approvals: u8,
        threshold: u8,
    },
    TimelockActive {
        ready_at_ns: u64,
    },
    ChunksMissing(Vec<u32>),
    /// `canister` can spend `available` cycles above its freezing reserve,
    /// and the execution needs `needed`. Anyone can top it up.
    InsufficientCycles {
        canister: Principal,
        available: Nat,
        needed: Nat,
    },
    /// Another execution runs; `None` while the controller only starts the
    /// wallet again.
    ExecutionInProgress {
        operation: Option<u64>,
    },
    /// A step of the execution failed; the wallet runs again and the
    /// operation stays open.
    ExecutionFailed(String),
    /// The wallet was handed to another controller.
    Replaced(Principal),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subnet() -> Principal {
        Principal::from_slice(&[9; 29])
    }

    /// A controller created with the IC's default settings.
    fn defaults() -> Health {
        Health {
            subnet: subnet(),
            controllers: vec![],
            freezing_threshold: Nat::from(2_592_000u64),
            reserved_cycles_limit: Nat::from(5_000_000_000_000u64),
            wasm_memory_limit: Nat::from(3_221_225_472u64),
            memory_allocation: Nat::from(0u8),
            compute_allocation: Nat::from(0u8),
            cycles: Nat::from(1_000_000_000_000u64),
            idle_cycles_burned_per_day: Nat::from(30_000_000u64),
        }
    }

    #[test]
    fn a_controller_with_default_settings_on_the_wallets_subnet_passes() {
        assert_eq!(defaults().check(subnet()), Ok(()));
        let unlimited = Health {
            wasm_memory_limit: Nat::from(0u8),
            freezing_threshold: Nat::from(MAX_FREEZING_THRESHOLD_S),
            ..defaults()
        };
        assert_eq!(unlimited.check(subnet()), Ok(()));
    }

    #[test]
    fn settings_that_could_strand_the_wallet_fail() {
        let elsewhere = Principal::from_slice(&[8; 29]);
        assert!(defaults().check(elsewhere).unwrap_err().contains("subnet"));
        let cases: Vec<(&str, Health)> = vec![
            (
                "controllers",
                Health {
                    controllers: vec![Principal::anonymous()],
                    ..defaults()
                },
            ),
            (
                "freezing threshold",
                Health {
                    freezing_threshold: Nat::from(MIN_FREEZING_THRESHOLD_S - 1),
                    ..defaults()
                },
            ),
            (
                "freezing threshold",
                Health {
                    freezing_threshold: Nat::from(MAX_FREEZING_THRESHOLD_S + 1),
                    ..defaults()
                },
            ),
            (
                "freezing threshold",
                Health {
                    freezing_threshold: Nat::from(u128::MAX) * Nat::from(2u8),
                    ..defaults()
                },
            ),
            (
                "allocation",
                Health {
                    memory_allocation: Nat::from(1u8),
                    ..defaults()
                },
            ),
            (
                "allocation",
                Health {
                    compute_allocation: Nat::from(1u8),
                    ..defaults()
                },
            ),
            (
                "reserved cycles",
                Health {
                    reserved_cycles_limit: Nat::from(0u8),
                    ..defaults()
                },
            ),
            (
                "Wasm memory",
                Health {
                    wasm_memory_limit: Nat::from(MIN_WASM_MEMORY_LIMIT - 1),
                    ..defaults()
                },
            ),
        ];
        for (expected, health) in cases {
            let reason = health.check(subnet()).unwrap_err();
            assert!(reason.contains(expected), "{expected}: {reason}");
        }
    }
}
