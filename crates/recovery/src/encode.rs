//! The fixed byte encodings that are hashed and signed. Every encoding starts
//! with an ASCII domain and a zero byte, so bytes signed for one purpose never
//! read as another, nor as an IC request: those start with `\x0Aic-request`
//! or `\x1Aic-request-auth-delegation`.
//!
//! Integers are big-endian, a byte string is its length as 4 big-endian
//! bytes followed by its bytes, and a principal is the byte string of its
//! raw bytes.

use crate::{InstallMode, Operation, OperationKind, Policy};
use candid::Principal;

pub const OPERATION_DOMAIN: &str = "b3wallet-controller/operation";
pub const POLICY_DOMAIN: &str = "b3wallet-controller/policy";
pub const APPROVE_DOMAIN: &str = "b3wallet-controller/approve";
pub const CANCEL_DOMAIN: &str = "b3wallet-controller/cancel";
pub const ENROLL_DOMAIN: &str = "b3wallet-controller/enroll";

/// The version of the operation encoding, its first byte after the domain.
const OPERATION_FORMAT: u8 = 1;

struct Encoder(Vec<u8>);

impl Encoder {
    fn new(domain: &str) -> Encoder {
        let mut bytes = domain.as_bytes().to_vec();
        bytes.push(0);
        Encoder(bytes)
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn len(&mut self, len: usize) {
        self.u32(u32::try_from(len).expect("an encoded length fits in 4 bytes"));
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.len(bytes.len());
        self.0.extend_from_slice(bytes);
    }

    fn principal(&mut self, principal: &Principal) {
        self.bytes(principal.as_slice());
    }

    fn policy(&mut self, policy: &Policy) {
        self.len(policy.authorities.len());
        for authority in &policy.authorities {
            self.bytes(&authority.public_key);
            self.bytes(authority.label.as_bytes());
        }
        self.u8(policy.normal_threshold);
        self.u8(policy.sovereign_threshold);
        self.u64(policy.normal_delay_ns);
        self.u64(policy.sovereign_delay_ns);
        self.len(policy.release_signers.len());
        for signer in &policy.release_signers {
            self.bytes(signer);
        }
        self.u8(policy.release_threshold);
    }
}

/// `POLICY_DOMAIN`, a zero byte, then the policy: the number of keys, each
/// key and label, the normal and sovereign thresholds (one byte each), the
/// normal and sovereign delays (8 bytes each), the number of release
/// signers, each signer key, and the release threshold (one byte).
pub fn policy_preimage(policy: &Policy) -> Vec<u8> {
    let mut encoder = Encoder::new(POLICY_DOMAIN);
    encoder.policy(policy);
    encoder.0
}

/// `OPERATION_DOMAIN`, a zero byte, the format byte 1, the controller, the
/// wallet, the policy version, the nonce and the expiry (8 bytes each), then
/// the kind:
///
/// - `InstallCode`: 1; the mode, 1 and then 1 or 0 for `skip_pre_upgrade`,
///   or 2 for a reinstall; the Wasm hash; the number of chunks and each
///   chunk hash; the SHA-256 of the argument; and the route: 0 for the
///   sovereign route, or 1 for the release route followed by the release's
///   version, Wasm size (8 bytes), number of `tested_from` hashes and each
///   hash, notes URL and publication time (8 bytes). The release's module
///   and chunk hashes are the install's own; its signatures are checked,
///   not signed.
/// - `SetPolicy`: 2, then the policy as in `policy_preimage`, without its
///   domain. The proofs are checked, not signed.
/// - `ReplaceController`: 3, the new controller, its module hash and its
///   policy hash.
pub fn operation_preimage(operation: &Operation) -> Vec<u8> {
    let mut encoder = Encoder::new(OPERATION_DOMAIN);
    encoder.u8(OPERATION_FORMAT);
    encoder.principal(&operation.controller);
    encoder.principal(&operation.wallet);
    encoder.u64(operation.policy_version);
    encoder.u64(operation.nonce);
    encoder.u64(operation.expires_at_ns);
    match &operation.kind {
        OperationKind::InstallCode(install) => {
            encoder.u8(1);
            match install.mode {
                InstallMode::Upgrade { skip_pre_upgrade } => {
                    encoder.u8(1);
                    encoder.u8(u8::from(skip_pre_upgrade));
                }
                InstallMode::Reinstall => encoder.u8(2),
            }
            encoder.bytes(&install.wasm_sha256);
            encoder.len(install.chunk_hashes.len());
            for hash in &install.chunk_hashes {
                encoder.bytes(hash);
            }
            encoder.bytes(&crate::sha256(&install.arg));
            match &install.release {
                Some(manifest) => {
                    encoder.u8(1);
                    encoder.bytes(manifest.version.as_bytes());
                    encoder.u64(manifest.wasm_size);
                    encoder.len(manifest.tested_from.len());
                    for hash in &manifest.tested_from {
                        encoder.bytes(hash);
                    }
                    encoder.bytes(manifest.notes_url.as_bytes());
                    encoder.u64(manifest.published_at_ns);
                }
                None => encoder.u8(0),
            }
        }
        OperationKind::SetPolicy { policy, .. } => {
            encoder.u8(2);
            encoder.policy(policy);
        }
        OperationKind::ReplaceController {
            controller,
            module_hash,
            policy_sha256,
        } => {
            encoder.u8(3);
            encoder.principal(controller);
            encoder.bytes(module_hash);
            encoder.bytes(policy_sha256);
        }
    }
    encoder.0
}

/// `domain`, a zero byte and the 32-byte digest, as raw bytes.
pub fn message(domain: &str, digest: &[u8; 32]) -> Vec<u8> {
    let mut encoder = Encoder::new(domain);
    encoder.0.extend_from_slice(digest);
    encoder.0
}

/// `ENROLL_DOMAIN`, a zero byte, the controller, the wallet and the key.
pub fn enroll_preimage(controller: Principal, wallet: Principal, key: &[u8]) -> Vec<u8> {
    let mut encoder = Encoder::new(ENROLL_DOMAIN);
    encoder.principal(&controller);
    encoder.principal(&wallet);
    encoder.bytes(key);
    encoder.0
}
