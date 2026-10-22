//! Operations of a wallet's recovery controller ([decision
//! 0009](../../docs/decisions/0009-recovery-controller.md)): the policy of
//! recovery keys, the operations they approve, the bytes a key signs, and the
//! checks the controller runs before it counts a signature. The controller and
//! `recovery-tool` share this crate, so an operation the tool signs is one the
//! controller reads the same way.
//!
//! An operation's digest is SHA-256 over a fixed byte encoding, not over its
//! Candid encoding, which other encoders lay out differently: a client in
//! another language computes the same digest from the rules in `encode`, which
//! `docs/recovery.md` spells out with test vectors.

pub mod controller;
mod encode;
pub mod history;

pub use encode::{APPROVE_DOMAIN, CANCEL_DOMAIN, ENROLL_DOMAIN, OPERATION_DOMAIN, POLICY_DOMAIN};

use candid::{CandidType, Principal};
use ed25519_dalek::Signature;
use release::{MAX_CHUNKS, MAX_SIGNERS, ReleaseManifest, Signers};
use serde::Deserialize;
use serde_bytes::ByteBuf;

pub use release::sha256;

pub const HOUR_NS: u64 = 3_600 * 1_000_000_000;
pub const DAY_NS: u64 = 24 * HOUR_NS;

/// The fewest recovery keys a policy holds: with two or more keys needed
/// for anything but a signed release, and one key that may be lost, a
/// policy needs three.
pub const MIN_AUTHORITIES: usize = 3;
/// The most recovery keys a policy holds.
pub const MAX_AUTHORITIES: usize = 10;
/// The longest label of a key, in characters.
pub const MAX_LABEL_LEN: usize = 64;
/// The shortest delay of the release route, whatever the policy says.
pub const MIN_NORMAL_DELAY_NS: u64 = 24 * HOUR_NS;
/// The shortest delay of the sovereign route, whatever the policy says.
pub const MIN_SOVEREIGN_DELAY_NS: u64 = 72 * HOUR_NS;
/// The longest delay a policy may set.
pub const MAX_DELAY_NS: u64 = 30 * DAY_NS;
/// How far ahead an operation may expire: the longest delay, and 30 days to
/// gather its approvals and execute it.
pub const MAX_LIFETIME_NS: u64 = MAX_DELAY_NS + 30 * DAY_NS;
/// The largest install argument an operation carries.
pub const MAX_ARG_LEN: usize = 64 * 1024;

/// One recovery key: an Ed25519 public key the owner holds outside the
/// wallet, and a label. Labels are public, like the whole policy.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    pub public_key: ByteBuf,
    pub label: String,
}

/// Who approves what, and after how long. The release route installs a
/// release that `release_threshold` of `release_signers` signed; the
/// sovereign route is every other operation.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub authorities: Vec<Authority>,
    pub normal_threshold: u8,
    pub sovereign_threshold: u8,
    pub normal_delay_ns: u64,
    pub sovereign_delay_ns: u64,
    /// May be empty, with a threshold of 0: then there is no release route.
    pub release_signers: Vec<ByteBuf>,
    pub release_threshold: u8,
}

/// A key's signature over `enroll_message`: proof that whoever enrolls the
/// key holds it, for this controller and this wallet.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub public_key: ByteBuf,
    pub signature: ByteBuf,
}

/// A key's signature over an operation's `approve_message` or
/// `cancel_message`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub public_key: ByteBuf,
    pub signature: ByteBuf,
}

/// How the IC installs the code. An upgrade keeps the wallet's stable
/// memory; a reinstall erases it.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMode {
    Upgrade { skip_pre_upgrade: bool },
    Reinstall,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InstallCode {
    pub mode: InstallMode,
    /// The SHA-256 of the whole module, which the IC checks at install.
    pub wasm_sha256: ByteBuf,
    /// The SHA-256 of each chunk of at most 1 MiB, in install order.
    pub chunk_hashes: Vec<ByteBuf>,
    /// The install argument, passed to `post_upgrade` or `init`.
    pub arg: ByteBuf,
    /// A release the policy's release signers signed for this module: the
    /// release route. Only with a plain upgrade.
    pub release: Option<ReleaseManifest>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum OperationKind {
    InstallCode(InstallCode),
    /// Replaces the whole policy. `proofs` holds a proof for every key the
    /// current policy does not hold.
    SetPolicy {
        policy: Policy,
        proofs: Vec<Proof>,
    },
    /// Hands the wallet to another controller, which must run the module
    /// `module_hash`, have no controller, and serve this wallet with the
    /// policy whose `Policy::sha256` is `policy_sha256`.
    ReplaceController {
        controller: Principal,
        module_hash: ByteBuf,
        policy_sha256: ByteBuf,
    },
}

/// What recovery keys approve. Every field is in the digest.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub controller: Principal,
    pub wallet: Principal,
    /// The policy version it is approved under; a policy change makes it
    /// stale.
    pub policy_version: u64,
    /// The controller's next nonce when it is proposed; also its id.
    pub nonce: u64,
    pub expires_at_ns: u64,
    pub kind: OperationKind,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// An upgrade to a release the policy's release signers signed.
    Release,
    /// Everything else.
    Sovereign,
}

/// What an operation needs before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub threshold: u8,
    pub delay_ns: u64,
}

impl Policy {
    /// Checks every rule the controller enforces whatever the owners chose:
    /// the keys, the thresholds, the delays and the release signers.
    pub fn validate(&self) -> Result<(), String> {
        let keys = self.authorities.len();
        if !(MIN_AUTHORITIES..=MAX_AUTHORITIES).contains(&keys) {
            return Err(format!(
                "a policy holds {MIN_AUTHORITIES} to {MAX_AUTHORITIES} recovery keys, not {keys}"
            ));
        }
        for (index, authority) in self.authorities.iter().enumerate() {
            if release::public_key(&authority.public_key).is_none() {
                return Err(format!(
                    "recovery key {index} is not an Ed25519 public key, or is a weak one"
                ));
            }
            let earlier = &self.authorities[..index];
            if earlier
                .iter()
                .any(|other| other.public_key == authority.public_key)
            {
                return Err(format!("recovery key {index} is listed twice"));
            }
            check_label(&authority.label)
                .map_err(|reason| format!("the label of recovery key {index} {reason}"))?;
            if earlier
                .iter()
                .any(|other| other.label.eq_ignore_ascii_case(&authority.label))
            {
                return Err(format!(
                    "the label of recovery key {index} is another key's label"
                ));
            }
        }
        let (normal, sovereign) = (
            usize::from(self.normal_threshold),
            usize::from(self.sovereign_threshold),
        );
        if sovereign < 2 || sovereign > keys - 1 {
            return Err(format!(
                "with {keys} keys the sovereign threshold is 2 to {}, so one stolen key never \
                 installs code or changes the policy, and one lost key never blocks a repair or \
                 a key rotation",
                keys - 1
            ));
        }
        if normal == 0 || normal >= sovereign {
            return Err(format!(
                "the normal threshold is 1 to {}, below the sovereign threshold",
                sovereign - 1
            ));
        }
        if self.normal_delay_ns < MIN_NORMAL_DELAY_NS {
            return Err("the normal delay must be at least 24 hours".to_string());
        }
        if self.sovereign_delay_ns < MIN_SOVEREIGN_DELAY_NS {
            return Err("the sovereign delay must be at least 72 hours".to_string());
        }
        if self.sovereign_delay_ns <= self.normal_delay_ns {
            return Err("the sovereign delay must be longer than the normal delay".to_string());
        }
        if self.normal_delay_ns > MAX_DELAY_NS || self.sovereign_delay_ns > MAX_DELAY_NS {
            return Err("a delay must be at most 30 days".to_string());
        }
        if self.release_signers.is_empty() {
            if self.release_threshold != 0 {
                return Err("with no release signers the release threshold must be 0".to_string());
            }
        } else {
            Signers::new(&self.release_signers, self.release_threshold)?;
            // A key that both signs releases and approves them would make a
            // signed release, and so any module, one key's choice.
            if let Some(index) = self
                .release_signers
                .iter()
                .position(|signer| self.holds(signer))
            {
                return Err(format!(
                    "release signer {index} is also a recovery key; the two sets must be apart"
                ));
            }
        }
        Ok(())
    }

    /// Whether `key` is one of the policy's recovery keys.
    pub fn holds(&self, key: &[u8]) -> bool {
        self.authorities
            .iter()
            .any(|authority| authority.public_key.as_slice() == key)
    }

    /// The recovery key whose self-authenticating principal is `caller`.
    pub fn authority_of(&self, caller: &Principal) -> Option<&Authority> {
        self.authorities
            .iter()
            .find(|authority| principal_of(&authority.public_key) == Some(*caller))
    }

    /// The threshold and delay of a route.
    pub fn requirement(&self, route: Route) -> Requirement {
        match route {
            Route::Release => Requirement {
                threshold: self.normal_threshold,
                delay_ns: self.normal_delay_ns,
            },
            Route::Sovereign => Requirement {
                threshold: self.sovereign_threshold,
                delay_ns: self.sovereign_delay_ns,
            },
        }
    }

    /// The release signers, or `None` when the policy has none.
    pub fn signers(&self) -> Option<Signers> {
        if self.release_signers.is_empty() {
            return None;
        }
        Signers::new(&self.release_signers, self.release_threshold).ok()
    }

    /// The SHA-256 that names this policy: a wallet hands itself over only to
    /// a controller whose policy has the hash its owners approved.
    pub fn sha256(&self) -> [u8; 32] {
        sha256(&encode::policy_preimage(self))
    }
}

/// A label is printable ASCII, so no two labels look alike while they
/// differ, and no character reorders or hides the text around it.
fn check_label(label: &str) -> Result<(), String> {
    let printable = label.bytes().all(|byte| (b' '..=b'~').contains(&byte));
    if label.is_empty()
        || label.len() > MAX_LABEL_LEN
        || !printable
        || label.starts_with(' ')
        || label.ends_with(' ')
    {
        return Err(format!(
            "must be 1 to {MAX_LABEL_LEN} printable ASCII characters, with no space at either \
             end"
        ));
    }
    Ok(())
}

impl Operation {
    pub fn route(&self) -> Route {
        match &self.kind {
            OperationKind::InstallCode(install) if install.release.is_some() => Route::Release,
            _ => Route::Sovereign,
        }
    }

    /// The digest every approval and cancellation signs.
    pub fn digest(&self) -> [u8; 32] {
        sha256(&encode::operation_preimage(self))
    }

    /// Whether this operation takes `key` out of the policy. A key cannot
    /// cancel its own removal, so one stolen key cannot block the others for
    /// longer than the delay of removing it.
    pub fn removes(&self, key: &[u8]) -> bool {
        matches!(&self.kind, OperationKind::SetPolicy { policy, .. } if !policy.holds(key))
    }

    /// Checks the form of every field, without reading any state.
    pub fn check_form(&self) -> Result<(), String> {
        match &self.kind {
            OperationKind::InstallCode(install) => install.check_form(),
            OperationKind::SetPolicy { policy, proofs } => {
                policy.validate()?;
                if proofs.len() > MAX_AUTHORITIES {
                    return Err(format!("at most {MAX_AUTHORITIES} proofs"));
                }
                Ok(())
            }
            OperationKind::ReplaceController {
                controller,
                module_hash,
                policy_sha256,
            } => {
                if !is_canister_id(controller) {
                    return Err("the new controller must be a canister".to_string());
                }
                if module_hash.len() != 32 || policy_sha256.len() != 32 {
                    return Err("module_hash and policy_sha256 must be 32 bytes".to_string());
                }
                Ok(())
            }
        }
    }
}

impl InstallCode {
    fn check_form(&self) -> Result<(), String> {
        if self.wasm_sha256.len() != 32 {
            return Err("wasm_sha256 must be 32 bytes".to_string());
        }
        if self.chunk_hashes.is_empty() || self.chunk_hashes.len() > MAX_CHUNKS {
            return Err(format!("a module has 1 to {MAX_CHUNKS} chunks"));
        }
        if let Some(index) = self.chunk_hashes.iter().position(|hash| hash.len() != 32) {
            return Err(format!("chunk hash {index} must be 32 bytes"));
        }
        if self.arg.len() > MAX_ARG_LEN {
            return Err(format!(
                "the install argument is at most {MAX_ARG_LEN} bytes"
            ));
        }
        if let Some(manifest) = &self.release {
            if self.mode
                != (InstallMode::Upgrade {
                    skip_pre_upgrade: false,
                })
            {
                return Err("a release goes only with an upgrade that runs pre_upgrade".to_string());
            }
            manifest
                .check_form()
                .map_err(|reason| format!("the release is out of form: {reason}"))?;
            if manifest.wasm_sha256 != self.wasm_sha256
                || manifest.chunk_hashes != self.chunk_hashes
            {
                return Err(format!(
                    "release {} names another module than the operation",
                    manifest.version
                ));
            }
        }
        Ok(())
    }
}

/// What the controller knows when an operation is proposed.
pub struct Context<'a> {
    pub controller: Principal,
    pub wallet: Principal,
    pub policy: &'a Policy,
    pub policy_version: u64,
    pub next_nonce: u64,
    pub now: u64,
}

/// Checks an operation against the controller's state when it is proposed,
/// and returns its route. Signatures over it are checked apart.
pub fn check_proposal(operation: &Operation, context: &Context) -> Result<Route, String> {
    if operation.controller != context.controller {
        return Err(format!(
            "the operation is for controller {}, not {}",
            operation.controller, context.controller
        ));
    }
    if operation.wallet != context.wallet {
        return Err(format!(
            "the operation is for wallet {}, not {}",
            operation.wallet, context.wallet
        ));
    }
    if operation.policy_version != context.policy_version {
        return Err(format!(
            "the operation is for policy version {}, and the policy is at version {}",
            operation.policy_version, context.policy_version
        ));
    }
    if operation.nonce != context.next_nonce {
        return Err(format!(
            "the operation's nonce is {}, and the controller's next nonce is {}",
            operation.nonce, context.next_nonce
        ));
    }
    if operation.expires_at_ns > context.now.saturating_add(MAX_LIFETIME_NS) {
        return Err("the operation must expire within 60 days from now".to_string());
    }
    // An operation that expires before its delay could end can never run;
    // the controller would only keep it.
    let delay_ns = context.policy.requirement(operation.route()).delay_ns;
    if operation.expires_at_ns <= context.now.saturating_add(delay_ns) {
        return Err(format!(
            "the operation must expire after its delay of {} hours ends",
            delay_ns / HOUR_NS
        ));
    }
    operation.check_form()?;
    match &operation.kind {
        OperationKind::InstallCode(InstallCode {
            release: Some(manifest),
            ..
        }) => {
            let signers = context.policy.signers().ok_or_else(|| {
                "this controller trusts no release signers, so an install takes the sovereign \
                 route: leave the release out"
                    .to_string()
            })?;
            signers.check(manifest).map_err(|missing| {
                format!(
                    "release {} has {} of the {} signatures by this controller's release signers \
                     it needs",
                    manifest.version, missing.have, missing.need
                )
            })?;
        }
        OperationKind::InstallCode(_) => {}
        OperationKind::SetPolicy { policy, proofs } => {
            let enrolled: Vec<&[u8]> = context
                .policy
                .authorities
                .iter()
                .map(|authority| authority.public_key.as_slice())
                .collect();
            check_proofs(
                policy,
                proofs,
                &enrolled,
                context.controller,
                context.wallet,
            )?;
        }
        OperationKind::ReplaceController { controller, .. } => {
            if *controller == context.controller || *controller == context.wallet {
                return Err("the new controller must be another canister".to_string());
            }
        }
    }
    Ok(operation.route())
}

/// Checks that `proofs` holds a valid proof of possession for every key of
/// `policy` that `enrolled` does not hold. A proof for a key outside the
/// policy, or a second proof for a key, is refused.
pub fn check_proofs(
    policy: &Policy,
    proofs: &[Proof],
    enrolled: &[&[u8]],
    controller: Principal,
    wallet: Principal,
) -> Result<(), String> {
    for (index, proof) in proofs.iter().enumerate() {
        if !policy.holds(&proof.public_key) {
            return Err(format!(
                "proof {index} is for a key the policy does not hold"
            ));
        }
        if proofs[..index]
            .iter()
            .any(|other| other.public_key == proof.public_key)
        {
            return Err(format!("proof {index} repeats a key"));
        }
    }
    for authority in &policy.authorities {
        let key = authority.public_key.as_slice();
        if enrolled.contains(&key) {
            continue;
        }
        let proof = proofs
            .iter()
            .find(|proof| proof.public_key.as_slice() == key)
            .ok_or_else(|| {
                format!(
                    "recovery key \"{}\" needs a proof of possession",
                    authority.label
                )
            })?;
        if !verify(
            key,
            &enroll_message(controller, wallet, key),
            &proof.signature,
        ) {
            return Err(format!(
                "the proof of possession of recovery key \"{}\" does not verify for this \
                 controller and wallet",
                authority.label
            ));
        }
    }
    Ok(())
}

/// What a key signs to approve the operation with `digest`.
pub fn approve_message(digest: &[u8; 32]) -> Vec<u8> {
    encode::message(APPROVE_DOMAIN, digest)
}

/// What a key signs to cancel the operation with `digest`.
pub fn cancel_message(digest: &[u8; 32]) -> Vec<u8> {
    encode::message(CANCEL_DOMAIN, digest)
}

/// What a key signs to prove, at enrollment, that its holder has it.
pub fn enroll_message(controller: Principal, wallet: Principal, key: &[u8]) -> Vec<u8> {
    encode::enroll_preimage(controller, wallet, key)
}

/// Strict Ed25519 verification: a key that is a curve point and not weak,
/// canonical encodings, and no small-order signature point, so one signature
/// cannot be reshaped into another.
pub fn verify(key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Some(key) = release::public_key(key) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify_strict(message, &signature).is_ok()
}

/// Whether `principal` is a canister id: an opaque id, whose last byte is 1.
/// Users, the anonymous principal and the management canister are not.
pub fn is_canister_id(principal: &Principal) -> bool {
    principal.as_slice().last() == Some(&1)
}

/// The DER prefix of an Ed25519 public key (RFC 8410).
const ED25519_DER_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// The self-authenticating principal of an Ed25519 public key: the caller
/// the IC names when that key signs a message. `None` for anything that is
/// not 32 bytes.
pub fn principal_of(key: &[u8]) -> Option<Principal> {
    if key.len() != 32 {
        return None;
    }
    let mut der = ED25519_DER_PREFIX.to_vec();
    der.extend_from_slice(key);
    Some(Principal::self_authenticating(der))
}

/// The most signatures one call carries: one per key.
pub const MAX_SIGNATURES: usize = MAX_AUTHORITIES;

const _: () = assert!(MAX_AUTHORITIES <= MAX_SIGNERS);

#[cfg(test)]
mod tests;
