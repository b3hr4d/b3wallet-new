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
    /// The Bitcoin network of the wallet's default Bitcoin account, and of
    /// its threshold keys. None is mainnet.
    pub bitcoin_network: Option<BitcoinNetwork>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct UpgradeArgs {
    /// Sets the network of a wallet that has no Bitcoin or EVM account yet;
    /// once it has one, an upgrade never changes it.
    pub bitcoin_network: Option<BitcoinNetwork>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Owner,
    Approver,
    Viewer,
    /// Reads, and proposes transfers that wait for an owner or an approver:
    /// its own approval never counts. For an AI agent or a script.
    Proposer,
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

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Regtest,
}

/// An asset, named by where it lives: the ICP ledger, another ICRC-1
/// ledger, a Bitcoin network, or an EVM chain's native token.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Chain {
    Icp,
    Icrc(Principal),
    Bitcoin(BitcoinNetwork),
    Evm(u64),
}

/// What an account is, which says what it holds.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    /// An ICRC-1 subaccount of the wallet's principal: ICP and every tracked
    /// ICRC-1 token.
    InternetComputer,
    /// The P2WPKH address of a threshold ECDSA key: bitcoin of its network.
    Bitcoin(BitcoinNetwork),
    /// The address of a threshold ECDSA key: the native token of every
    /// tracked EVM chain.
    Evm,
}

pub type AccountId = u32;
pub type ActionId = u64;
pub type RequestId = u64;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AccountHolder {
    Wallet,
    Extension(Principal),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub kind: AccountKind,
    pub label: String,
    pub index: u32,
    pub holder: AccountHolder,
    pub address: String,
    /// An Internet Computer account's ICP account identifier, in hex.
    pub icp_account_id: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AddAccountArgs {
    pub kind: AccountKind,
    pub label: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Balance {
    pub account: AccountId,
    pub asset: Chain,
    pub amount: Nat,
    pub as_of_ns: u64,
}

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
    /// What it sends. Absent only from transfers proposed before accounts
    /// had kinds; `propose` stores it.
    pub asset: Option<Chain>,
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
    /// Makes `controller`, a recovery controller (decision 0009), the
    /// wallet's only controller, under the `SetPolicy` rule. After it the
    /// wallet cannot change its own code or controllers again.
    HandOverControl {
        controller: Principal,
        controller_module_hash: ByteBuf,
        controller_policy_sha256: ByteBuf,
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
    /// The owner's request a transfer answers (`propose_for`), whose words the guard checks it against.
    pub request: Option<RequestId>,
}

/// What an owner asked their agent to do, in their own words (decision 0012): the agent reads it with `requests`
/// and answers it with `propose_for`, and the guard checks the transfer against these words.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub text: String,
    pub asked_by: Principal,
    pub asked_at_ns: u64,
    pub open: bool,
    /// The actions proposed for it, oldest first.
    pub answers: Vec<ActionId>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AuditEvent {
    Claimed,
    JoinRequested(Principal),
    AccountAdded(AccountId),
    AssetTracked(Chain),
    AssetUntracked(Chain),
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
    Asked(RequestId),
    RequestClosed(RequestId),
    /// The guard file's model release, loaded by an owner.
    GuardLoaded(String),
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

/// What `http_request` answers. The request is `ic_http_certification`'s
/// `HttpRequest`, whose Candid type is the one in `candid/wallet.did`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub headers: Vec<(String, String)>,
    pub body: ByteBuf,
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
            // Giving up control is approved like the policy, by the
            // strongest rule.
            Action::SetPolicy(_) | Action::HandOverControl { .. } => ActionKind::SetPolicy,
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
    InsufficientCycles { available: Nat, needed: Nat },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum WalletArg {
    Init(InitArgs),
    Upgrade(Option<UpgradeArgs>),
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
    pub bitcoin_network: BitcoinNetwork,
}

/// Bytes of the guard file (`guard::FILE_BYTES` long) written at `offset`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GuardUploadArgs {
    pub offset: u64,
    pub chunk: ByteBuf,
}

/// The guard file this release pins, and what is loaded.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GuardStatus {
    pub file_sha256: ByteBuf,
    pub file_bytes: u64,
    pub loaded: bool,
    /// The model's release and the SHA-256 of its weights, from the loaded file.
    pub release: Option<String>,
    pub model_sha256: Option<String>,
}

/// Where the guard read a slot in the request: `[start, end)` in code points, and the words.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GuardSpan {
    pub slot: String,
    pub start: u32,
    pub end: u32,
    pub text: String,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardVerdict {
    Same,
    Differs,
    NotChecked,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GuardField {
    pub field: String,
    pub verdict: GuardVerdict,
}

/// Differs: a field differs or the request raised a flag. Matches: amount, asset and recipient are the same.
/// Partial: nothing differs, but not all three could be checked.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardOverall {
    Differs,
    Matches,
    Partial,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GuardMatch {
    pub cue: String,
    pub start: u32,
    pub end: u32,
    pub text: String,
}

/// A text the approver reads (the proposer's label, the memo), through the keyword list.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GuardText {
    pub name: String,
    pub text: String,
    pub flags: Vec<String>,
    pub matched: Vec<GuardMatch>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
pub struct GuardReport {
    /// The owner's words it checked: the request the transfer answers.
    pub request: String,
    pub class: String,
    pub abstain: bool,
    pub confidence: f32,
    pub spans: Vec<GuardSpan>,
    pub fields: Vec<GuardField>,
    pub request_flags: Vec<String>,
    pub overall: GuardOverall,
    pub texts: Vec<GuardText>,
    pub instructions: u64,
}

/// Canister ids are opaque principals, whose last byte is 0x01.
pub fn is_canister_id(id: &Principal) -> bool {
    id.as_slice().last() == Some(&0x01)
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
        if !is_canister_id(&self.release_channel) {
            return Err("release_channel must be a canister id".to_string());
        }
        // The same keys check every release the wallet installs.
        release::Signers::new(&self.release_signers, self.release_threshold)?;
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
            bitcoin_network: None,
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
    fn rejects_more_than_sixteen_signer_keys() {
        let args = InitArgs {
            release_signers: (1..=17).map(key).collect(),
            ..valid()
        };
        assert!(args.validate(NOW).unwrap_err().contains("at most 16"));
        let args = InitArgs {
            release_signers: (1..=16).map(key).collect(),
            ..valid()
        };
        assert_eq!(args.validate(NOW), Ok(()));
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
                    .contains("release threshold")
            );
        }
    }
}
