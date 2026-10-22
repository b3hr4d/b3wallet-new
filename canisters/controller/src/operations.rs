//! An operation from its proposal to its end: approvals, the delay, expiry,
//! cancellation, staleness and execution. Pure: the caller passes the time,
//! the policy and its version, and stores the result.

use candid::CandidType;
use recovery::controller::{ControllerError, OperationStatus};
use recovery::{Operation, OperationKind, Policy, Requirement, Route, Signed};
use serde::Deserialize;
use serde_bytes::ByteBuf;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum StoredState {
    Open,
    Executed { at_ns: u64 },
    Cancelled { by: ByteBuf, at_ns: u64 },
    Expired,
    Stale,
}

/// An install sent to the IC. A later execution that finds the change after
/// `changes_before` in the wallet's history made by this controller at
/// `sender_version`, installing the operation's module, learns it ran.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SentInstall {
    /// How many changes the IC had recorded for the wallet just before.
    pub changes_before: u64,
    /// The controller's canister version when it sent the install, which
    /// the IC records with the change it makes.
    pub sender_version: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredOperation {
    pub operation: Operation,
    pub digest: ByteBuf,
    pub route: Route,
    /// The threshold and delay of its route under the policy it was proposed
    /// under, which is the only policy it can run under.
    pub threshold: u8,
    pub delay_ns: u64,
    /// The keys whose approvals verified, in the order they came.
    pub approvals: Vec<ByteBuf>,
    /// The key of the caller who proposed it.
    pub proposed_by: ByteBuf,
    pub proposed_at_ns: u64,
    /// When the threshold was met and the delay started.
    pub approved_at_ns: Option<u64>,
    pub state: StoredState,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// The install an execution sent and has not learned the answer to.
    pub install_sent: Option<SentInstall>,
    /// Whether an execution sent the hand-over of a `ReplaceController` and
    /// has not learned the answer. A later execution that finds the wallet
    /// controlled by the new controller alone learns it ran.
    pub hand_over_sent: bool,
}

impl StoredOperation {
    /// A new operation with the approvals in `signed`, which `verify_all`
    /// checked.
    pub fn new(
        operation: Operation,
        route: Route,
        requirement: Requirement,
        approvals: Vec<ByteBuf>,
        proposed_by: ByteBuf,
        now: u64,
    ) -> StoredOperation {
        let digest = ByteBuf::from(operation.digest().to_vec());
        let mut stored = StoredOperation {
            operation,
            digest,
            route,
            threshold: requirement.threshold,
            delay_ns: requirement.delay_ns,
            approvals: Vec::new(),
            proposed_by,
            proposed_at_ns: now,
            approved_at_ns: None,
            state: StoredState::Open,
            attempts: 0,
            last_error: None,
            install_sent: None,
            hand_over_sent: false,
        };
        stored.add_approvals(approvals, now);
        stored
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
            .as_slice()
            .try_into()
            .expect("a stored digest is 32 bytes")
    }

    pub fn is_open(&self) -> bool {
        self.state == StoredState::Open
    }

    fn add_approvals(&mut self, keys: Vec<ByteBuf>, now: u64) {
        self.approvals.extend(keys);
        if self.approved_at_ns.is_none() && self.approvals.len() >= usize::from(self.threshold) {
            self.approved_at_ns = Some(now);
        }
    }

    /// When its delay ends, once it has its threshold.
    pub fn ready_at(&self) -> Option<u64> {
        self.approved_at_ns
            .map(|at| at.saturating_add(self.delay_ns))
    }

    /// Ends an open operation whose policy version is no longer the current
    /// one, or whose expiry passed. Returns whether it changed.
    pub fn settle(&mut self, policy_version: u64, now: u64) -> bool {
        if !self.is_open() {
            return false;
        }
        if self.operation.policy_version != policy_version {
            self.state = StoredState::Stale;
            return true;
        }
        if now >= self.operation.expires_at_ns {
            self.state = StoredState::Expired;
            return true;
        }
        false
    }

    /// What the interface shows at `now`.
    pub fn status(&self, policy_version: u64, now: u64) -> OperationStatus {
        let mut current = self.clone();
        current.settle(policy_version, now);
        match current.state {
            StoredState::Executed { at_ns } => OperationStatus::Executed { at_ns },
            StoredState::Cancelled { by, at_ns } => OperationStatus::Cancelled { by, at_ns },
            StoredState::Expired => OperationStatus::Expired,
            StoredState::Stale => OperationStatus::Stale,
            StoredState::Open => match current.ready_at() {
                None => OperationStatus::Approving,
                Some(ready_at_ns) if now < ready_at_ns => OperationStatus::Waiting { ready_at_ns },
                Some(_) => OperationStatus::Ready,
            },
        }
    }

    /// Refuses unless the operation is open at `now`.
    pub fn check_open(&mut self, policy_version: u64, now: u64) -> Result<(), ControllerError> {
        self.settle(policy_version, now);
        if !self.is_open() {
            return Err(ControllerError::Closed(self.status(policy_version, now)));
        }
        Ok(())
    }

    /// Adds approvals that `verify_all` checked against the operation's
    /// digest. A key that approved already is refused.
    pub fn approve(
        &mut self,
        keys: Vec<ByteBuf>,
        policy_version: u64,
        now: u64,
    ) -> Result<(), ControllerError> {
        self.check_open(policy_version, now)?;
        if let Some(key) = keys.iter().find(|key| self.approvals.contains(key)) {
            return Err(ControllerError::InvalidArgument(format!(
                "key {} approved this operation already",
                hex(key)
            )));
        }
        self.add_approvals(keys, now);
        Ok(())
    }

    /// Cancels it for `key`, which `verify_all` checked signed the cancel
    /// message. A key cannot cancel an operation that removes it.
    pub fn cancel(
        &mut self,
        key: ByteBuf,
        policy_version: u64,
        now: u64,
    ) -> Result<(), ControllerError> {
        self.check_open(policy_version, now)?;
        if self.operation.removes(&key) {
            return Err(ControllerError::Forbidden(
                "a key cannot cancel the policy change that removes it".to_string(),
            ));
        }
        self.state = StoredState::Cancelled {
            by: key,
            at_ns: now,
        };
        Ok(())
    }

    /// Refuses unless it may run now: open, its threshold met and its delay
    /// passed.
    pub fn check_ready(&mut self, policy_version: u64, now: u64) -> Result<(), ControllerError> {
        self.check_open(policy_version, now)?;
        match self.ready_at() {
            None => Err(ControllerError::QuorumNotMet {
                approvals: u8::try_from(self.approvals.len()).unwrap_or(u8::MAX),
                threshold: self.threshold,
            }),
            Some(ready_at_ns) if now < ready_at_ns => {
                Err(ControllerError::TimelockActive { ready_at_ns })
            }
            Some(_) => Ok(()),
        }
    }

    pub fn executed(&mut self, now: u64) {
        self.state = StoredState::Executed { at_ns: now };
        self.last_error = None;
    }

    /// The `InstallCode` it carries, if it is one.
    pub fn install(&self) -> Option<&recovery::InstallCode> {
        match &self.operation.kind {
            OperationKind::InstallCode(install) => Some(install),
            _ => None,
        }
    }
}

/// Checks each signature over `message` by a key of `policy`, and returns the
/// keys. Refuses a key the policy does not hold, a signature that does not
/// verify, and a key that signs twice.
pub fn verify_all(
    policy: &Policy,
    message: &[u8],
    signed: &[Signed],
) -> Result<Vec<ByteBuf>, ControllerError> {
    if signed.is_empty() {
        return Err(ControllerError::InvalidArgument(
            "at least one signature is needed".to_string(),
        ));
    }
    if signed.len() > recovery::MAX_SIGNATURES {
        return Err(ControllerError::InvalidArgument(format!(
            "at most {} signatures",
            recovery::MAX_SIGNATURES
        )));
    }
    let mut keys: Vec<ByteBuf> = Vec::with_capacity(signed.len());
    for signature in signed {
        if !policy.holds(&signature.public_key) {
            return Err(ControllerError::BadSignature(format!(
                "key {} is not one of this controller's recovery keys",
                hex(&signature.public_key)
            )));
        }
        if keys.contains(&signature.public_key) {
            return Err(ControllerError::InvalidArgument(format!(
                "key {} signs twice",
                hex(&signature.public_key)
            )));
        }
        if !recovery::verify(&signature.public_key, message, &signature.signature) {
            return Err(ControllerError::BadSignature(format!(
                "the signature of key {} does not verify",
                hex(&signature.public_key)
            )));
        }
        keys.push(signature.public_key.clone());
    }
    Ok(keys)
}

/// A key's first eight bytes in hex, as the recovery tool prints them.
pub fn hex(key: &[u8]) -> String {
    key.iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;
    use ed25519_dalek::{Signer, SigningKey};
    use recovery::{Authority, DAY_NS, HOUR_NS, InstallCode, InstallMode};

    const T0: u64 = 1_000 * DAY_NS;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn public(seed: u8) -> ByteBuf {
        ByteBuf::from(key(seed).verifying_key().to_bytes())
    }

    fn policy() -> Policy {
        Policy {
            authorities: (1..=3)
                .map(|seed| Authority {
                    public_key: public(seed),
                    label: format!("key {seed}"),
                })
                .collect(),
            normal_threshold: 1,
            sovereign_threshold: 2,
            normal_delay_ns: 48 * HOUR_NS,
            sovereign_delay_ns: 7 * DAY_NS,
            release_signers: vec![],
            release_threshold: 0,
        }
    }

    fn operation(kind: OperationKind) -> Operation {
        Operation {
            controller: Principal::from_slice(&[1]),
            wallet: Principal::from_slice(&[2]),
            policy_version: 1,
            nonce: 0,
            expires_at_ns: T0 + 20 * DAY_NS,
            kind,
        }
    }

    fn repair() -> Operation {
        operation(OperationKind::InstallCode(InstallCode {
            mode: InstallMode::Upgrade {
                skip_pre_upgrade: false,
            },
            wasm_sha256: ByteBuf::from(vec![1; 32]),
            chunk_hashes: vec![ByteBuf::from(vec![1; 32])],
            arg: ByteBuf::new(),
            release: None,
        }))
    }

    fn approval(seed: u8, op: &Operation) -> Signed {
        Signed {
            public_key: public(seed),
            signature: ByteBuf::from(
                key(seed)
                    .sign(&recovery::approve_message(&op.digest()))
                    .to_bytes(),
            ),
        }
    }

    fn stored(op: Operation, approvers: &[u8]) -> StoredOperation {
        let policy = policy();
        let signed: Vec<Signed> = approvers.iter().map(|&seed| approval(seed, &op)).collect();
        let message = recovery::approve_message(&op.digest());
        let keys = verify_all(&policy, &message, &signed).unwrap();
        let route = op.route();
        StoredOperation::new(op, route, policy.requirement(route), keys, public(1), T0)
    }

    #[test]
    fn the_delay_starts_when_the_threshold_is_met() {
        let mut op = stored(repair(), &[1]);
        assert_eq!(op.status(1, T0), OperationStatus::Approving);
        assert_eq!(
            op.check_ready(1, T0),
            Err(ControllerError::QuorumNotMet {
                approvals: 1,
                threshold: 2
            })
        );
        op.approve(vec![public(2)], 1, T0 + DAY_NS).unwrap();
        let ready_at_ns = T0 + 8 * DAY_NS;
        assert_eq!(
            op.status(1, T0 + DAY_NS),
            OperationStatus::Waiting { ready_at_ns }
        );
        assert_eq!(
            op.check_ready(1, ready_at_ns - 1),
            Err(ControllerError::TimelockActive { ready_at_ns })
        );
        assert_eq!(op.check_ready(1, ready_at_ns), Ok(()));
        assert_eq!(op.status(1, ready_at_ns), OperationStatus::Ready);
    }

    #[test]
    fn a_key_approves_once() {
        let mut op = stored(repair(), &[1]);
        assert!(op.approve(vec![public(1)], 1, T0).is_err());
        assert_eq!(op.approvals.len(), 1);
        let digest = op.digest();
        let message = recovery::approve_message(&digest);
        let twice = [approval(2, &op.operation), approval(2, &op.operation)];
        assert!(verify_all(&policy(), &message, &twice).is_err());
    }

    #[test]
    fn signatures_must_be_by_policy_keys_over_this_digest() {
        let op = repair();
        let message = recovery::approve_message(&op.digest());
        assert!(matches!(
            verify_all(&policy(), &message, &[approval(4, &op)]),
            Err(ControllerError::BadSignature(_))
        ));
        let mut other = repair();
        other.nonce = 1;
        assert!(matches!(
            verify_all(&policy(), &message, &[approval(1, &other)]),
            Err(ControllerError::BadSignature(_))
        ));
        assert!(verify_all(&policy(), &message, &[]).is_err());
        assert_eq!(
            verify_all(&policy(), &message, &[approval(2, &op), approval(1, &op)]),
            Ok(vec![public(2), public(1)])
        );
    }

    #[test]
    fn an_operation_expires_and_goes_stale() {
        let mut op = stored(repair(), &[1, 2]);
        let expiry = op.operation.expires_at_ns;
        assert_eq!(op.status(1, expiry), OperationStatus::Expired);
        assert_eq!(op.status(2, T0), OperationStatus::Stale);
        assert_eq!(
            op.check_ready(1, expiry),
            Err(ControllerError::Closed(OperationStatus::Expired))
        );
        assert_eq!(op.state, StoredState::Expired);
        assert!(op.approve(vec![public(3)], 1, expiry).is_err());

        let mut stale = stored(repair(), &[1, 2]);
        assert!(stale.settle(2, T0));
        assert_eq!(stale.state, StoredState::Stale);
        assert!(stale.cancel(public(1), 2, T0).is_err());
    }

    #[test]
    fn any_key_cancels_except_its_own_removal() {
        let mut op = stored(repair(), &[1, 2]);
        op.cancel(public(3), 1, T0).unwrap();
        assert_eq!(
            op.state,
            StoredState::Cancelled {
                by: public(3),
                at_ns: T0
            }
        );
        assert!(op.check_ready(1, T0 + 30 * DAY_NS).is_err());

        let mut next = policy();
        next.authorities.remove(2);
        next.sovereign_threshold = 1;
        let mut removal = stored(
            operation(OperationKind::SetPolicy {
                policy: next,
                proofs: vec![],
            }),
            &[1, 2],
        );
        assert!(matches!(
            removal.cancel(public(3), 1, T0),
            Err(ControllerError::Forbidden(_))
        ));
        assert!(removal.is_open());
        removal.cancel(public(1), 1, T0).unwrap();
        assert!(!removal.is_open());
    }

    #[test]
    fn an_executed_operation_never_runs_again() {
        let mut op = stored(repair(), &[1, 2]);
        let ready = op.ready_at().unwrap();
        op.check_ready(1, ready).unwrap();
        op.executed(ready);
        assert_eq!(
            op.check_ready(1, ready + 1),
            Err(ControllerError::Closed(OperationStatus::Executed {
                at_ns: ready
            }))
        );
        assert!(op.cancel(public(1), 1, ready).is_err());
        // An executed operation does not go stale or expire afterwards.
        assert!(!op.settle(2, op.operation.expires_at_ns));
    }
}
