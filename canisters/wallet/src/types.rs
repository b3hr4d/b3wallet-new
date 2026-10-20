//! Rust mirrors of the types in `candid/wallet.did` that the implemented
//! methods use. The interface test checks them against that file.

use crate::ed25519;
use candid::{CandidType, Nat, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InitArgs {
    pub claim_public_key: ByteBuf,
    pub claim_expires_at_ns: u64,
    pub release_channel: Principal,
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
    pub factory: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Owner,
    Approver,
    Viewer,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: Principal,
    pub role: Role,
    pub label: String,
    pub added_at_ns: u64,
    pub added_by: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClaimArgs {
    pub signature: ByteBuf,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Regtest,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Chain {
    Icp,
    Icrc(Principal),
    Bitcoin(BitcoinNetwork),
    Evm(u64),
}

pub type AccountId = u32;
pub type ActionId = u64;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    ReadAccounts,
    ProposeTransfer,
    SpendSubWallet(Chain),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    pub chain: Chain,
    pub per_day: Nat,
    pub total: Option<Nat>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransferRequest {
    pub from: AccountId,
    pub to: String,
    pub amount: Nat,
    pub fee: Option<Nat>,
    pub memo: Option<ByteBuf>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Transfer(TransferRequest),
    AddMember {
        id: Principal,
        role: Role,
        label: String,
    },
    RemoveMember(Principal),
    ChangeRole {
        id: Principal,
        role: Role,
    },
    InstallExtension {
        canister: Principal,
        granted: Vec<Permission>,
        budgets: Vec<Budget>,
    },
    UninstallExtension(Principal),
    SetPolicy(Policy),
    Upgrade {
        version: String,
        wasm_sha256: ByteBuf,
    },
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionKind {
    Transfer,
    AddMember,
    RemoveMember,
    ChangeRole,
    InstallExtension,
    UninstallExtension,
    SetPolicy,
    Upgrade,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DailyLimit {
    pub chain: Chain,
    pub amount: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PolicyRule {
    pub kind: ActionKind,
    pub quorum: u8,
    pub timelock_ns: u64,
    pub daily_limits: Vec<DailyLimit>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub rules: Vec<PolicyRule>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TxReceipt {
    pub account: AccountId,
    pub chain: Chain,
    pub tx_id: String,
    pub at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Sent(TxReceipt),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ActionState {
    Waiting,
    Ready,
    Executed(Outcome),
    Rejected(Principal),
    Expired,
    Failed(String),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub id: ActionId,
    pub action: Action,
    pub proposed_by: Principal,
    pub proposed_at_ns: u64,
    pub approvals: Vec<Principal>,
    pub quorum: u8,
    pub executable_after_ns: u64,
    pub expires_at_ns: u64,
    pub state: ActionState,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AuditEvent {
    Claimed,
    JoinRequested(Principal),
    Proposed(ActionId),
    Approved(ActionId),
    Rejected(ActionId),
    Executed(ActionId),
    ExtensionCall {
        extension: Principal,
        request: String,
    },
    UpgradeStaged(String),
    UpgradeInstalled(String),
    UpgradeFailed(String),
    CyclesLow(Nat),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    pub seq: u64,
    pub at_ns: u64,
    pub by: Option<Principal>,
    pub event: AuditEvent,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditRequest {
    pub before_seq: Option<u64>,
    pub limit: u16,
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Action::Transfer(_) => ActionKind::Transfer,
            Action::AddMember { .. } => ActionKind::AddMember,
            Action::RemoveMember(_) => ActionKind::RemoveMember,
            Action::ChangeRole { .. } => ActionKind::ChangeRole,
            Action::InstallExtension { .. } => ActionKind::InstallExtension,
            Action::UninstallExtension(_) => ActionKind::UninstallExtension,
            Action::SetPolicy(_) => ActionKind::SetPolicy,
            Action::Upgrade { .. } => ActionKind::Upgrade,
        }
    }
}

impl ActionKind {
    pub const ALL: [ActionKind; 8] = [
        ActionKind::Transfer,
        ActionKind::AddMember,
        ActionKind::RemoveMember,
        ActionKind::ChangeRole,
        ActionKind::InstallExtension,
        ActionKind::UninstallExtension,
        ActionKind::SetPolicy,
        ActionKind::Upgrade,
    ];
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum WalletError {
    NotClaimed,
    AlreadyClaimed,
    ClaimExpired,
    BadSignature,
    NotAMember,
    Forbidden(String),
    NotFound(String),
    InvalidArgument(String),
    PolicyViolation(String),
    BudgetExceeded { chain: Chain, remaining: Nat },
    TimelockActive { executable_after_ns: u64 },
    QuorumNotMet { approvals: u8, quorum: u8 },
    UpgradeInProgress,
    ReleaseRejected(String),
    ChainError(String),
    InsufficientCycles,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum WalletArg {
    Init(InitArgs),
    Upgrade,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum UpgradeState {
    Idle,
    Staging {
        version: String,
        chunks_uploaded: u32,
        chunks_total: u32,
    },
    Installing {
        version: String,
        wasm_sha256: ByteBuf,
        started_at_ns: u64,
        deadline_ns: u64,
    },
    Failed {
        version: String,
        reason: String,
        at_ns: u64,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub version: String,
    pub claimed: bool,
    pub claim_expires_at_ns: Option<u64>,
    pub cycles: Nat,
    pub upgrade: UpgradeState,
    pub release_channel: Principal,
}

impl InitArgs {
    /// Checks the install arguments before anything is stored. `now_ns` is the
    /// IC time at install.
    pub fn validate(&self, now_ns: u64) -> Result<(), String> {
        if ed25519::public_key(&self.claim_public_key).is_none() {
            return Err("claim_public_key must be an Ed25519 public key".to_string());
        }
        if self.claim_expires_at_ns <= now_ns {
            return Err("claim_expires_at_ns must be in the future".to_string());
        }
        // Canister ids are opaque principals, whose last byte is 0x01.
        if self.release_channel.as_slice().last() != Some(&0x01) {
            return Err("release_channel must be a canister id".to_string());
        }
        if let Some(index) = self
            .release_signers
            .iter()
            .position(|key| ed25519::public_key(key).is_none())
        {
            return Err(format!(
                "release signer key {index} must be an Ed25519 public key"
            ));
        }
        let mut signers: Vec<&[u8]> = self.release_signers.iter().map(|k| k.as_slice()).collect();
        signers.sort_unstable();
        signers.dedup();
        if signers.len() != self.release_signers.len() {
            return Err("release_signers must not repeat a key".to_string());
        }
        if self.release_threshold == 0 || usize::from(self.release_threshold) > signers.len() {
            return Err(format!(
                "release_threshold must be between 1 and the number of release signers ({})",
                signers.len()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ed25519::tests::NOT_A_POINT;
    use ed25519_dalek::SigningKey;

    const NOW: u64 = 1_000;

    fn key(seed: u8) -> ByteBuf {
        let key = SigningKey::from_bytes(&[seed; 32]).verifying_key();
        ByteBuf::from(key.to_bytes().to_vec())
    }

    fn valid() -> InitArgs {
        InitArgs {
            claim_public_key: key(1),
            claim_expires_at_ns: NOW + 1,
            release_channel: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
            release_signers: vec![key(2), key(3)],
            release_threshold: 2,
            factory: None,
        }
    }

    #[test]
    fn accepts_valid_arguments() {
        assert_eq!(valid().validate(NOW), Ok(()));
    }

    #[test]
    fn rejects_a_claim_key_that_is_not_an_ed25519_key() {
        for claim_public_key in [vec![1; 31], NOT_A_POINT.to_vec()] {
            let args = InitArgs {
                claim_public_key: ByteBuf::from(claim_public_key),
                ..valid()
            };
            assert!(args.validate(NOW).unwrap_err().contains("claim_public_key"));
        }
    }

    #[test]
    fn rejects_a_claim_that_already_expired() {
        let args = InitArgs {
            claim_expires_at_ns: NOW,
            ..valid()
        };
        assert!(args.validate(NOW).unwrap_err().contains("future"));
    }

    #[test]
    fn rejects_a_release_channel_that_is_not_a_canister() {
        let user = Principal::self_authenticating([7; 32]);
        for release_channel in [
            Principal::anonymous(),
            Principal::management_canister(),
            user,
        ] {
            let args = InitArgs {
                release_channel,
                ..valid()
            };
            assert!(args.validate(NOW).unwrap_err().contains("release_channel"));
        }
    }

    #[test]
    fn rejects_a_malformed_signer_key() {
        for bad in [vec![3; 33], NOT_A_POINT.to_vec()] {
            let args = InitArgs {
                release_signers: vec![key(2), ByteBuf::from(bad)],
                ..valid()
            };
            assert!(args.validate(NOW).unwrap_err().contains("signer key 1"));
        }
    }

    #[test]
    fn rejects_a_repeated_signer_key() {
        let args = InitArgs {
            release_signers: vec![key(2), key(2)],
            release_threshold: 1,
            ..valid()
        };
        assert!(args.validate(NOW).unwrap_err().contains("repeat"));
    }

    #[test]
    fn rejects_a_threshold_outside_the_signer_count() {
        for release_threshold in [0, 3] {
            let args = InitArgs {
                release_threshold,
                ..valid()
            };
            assert!(
                args.validate(NOW)
                    .unwrap_err()
                    .contains("release_threshold")
            );
        }
    }
}
