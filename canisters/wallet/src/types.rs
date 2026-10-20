//! Rust mirrors of the types in `candid/wallet.did` that the implemented
//! methods use. The interface test checks them against that file.

use candid::{CandidType, Nat, Principal};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// Length of an Ed25519 public key, the claim key and each release signer key.
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;

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
        if self.claim_public_key.len() != ED25519_PUBLIC_KEY_LEN {
            return Err(format!(
                "claim_public_key must be {ED25519_PUBLIC_KEY_LEN} bytes, got {}",
                self.claim_public_key.len()
            ));
        }
        if self.claim_expires_at_ns <= now_ns {
            return Err("claim_expires_at_ns must be in the future".to_string());
        }
        // Canister ids are opaque principals, whose last byte is 0x01.
        if self.release_channel.as_slice().last() != Some(&0x01) {
            return Err("release_channel must be a canister id".to_string());
        }
        if let Some(key) = self
            .release_signers
            .iter()
            .find(|key| key.len() != ED25519_PUBLIC_KEY_LEN)
        {
            return Err(format!(
                "every release signer key must be {ED25519_PUBLIC_KEY_LEN} bytes, got {}",
                key.len()
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

    const NOW: u64 = 1_000;

    fn valid() -> InitArgs {
        InitArgs {
            claim_public_key: ByteBuf::from(vec![1; 32]),
            claim_expires_at_ns: NOW + 1,
            release_channel: Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
            release_signers: vec![ByteBuf::from(vec![2; 32]), ByteBuf::from(vec![3; 32])],
            release_threshold: 2,
            factory: None,
        }
    }

    #[test]
    fn accepts_valid_arguments() {
        assert_eq!(valid().validate(NOW), Ok(()));
    }

    #[test]
    fn rejects_a_short_claim_key() {
        let args = InitArgs {
            claim_public_key: ByteBuf::from(vec![1; 31]),
            ..valid()
        };
        assert!(args.validate(NOW).unwrap_err().contains("claim_public_key"));
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
        let args = InitArgs {
            release_signers: vec![ByteBuf::from(vec![2; 32]), ByteBuf::from(vec![3; 33])],
            ..valid()
        };
        assert!(args.validate(NOW).unwrap_err().contains("signer key"));
    }

    #[test]
    fn rejects_a_repeated_signer_key() {
        let args = InitArgs {
            release_signers: vec![ByteBuf::from(vec![2; 32]), ByteBuf::from(vec![2; 32])],
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
