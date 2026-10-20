//! The one-time claim that makes the first owner.
//!
//! The claim link carries the claim private key in its URL fragment. The
//! browser signs `claim:<wallet canister id>:<caller principal>` with it, so a
//! signature works only for the principal it names and only on this wallet.

use crate::WalletError;
use crate::ed25519;
use crate::storage::PendingClaim;
use candid::Principal;

/// The text the claim key signs.
pub fn message(wallet: Principal, caller: Principal) -> String {
    format!("claim:{}:{}", wallet.to_text(), caller.to_text())
}

/// Decides a claim. `pending` is `None` once the wallet is claimed, and
/// `has_members` is true from the first member on; either refuses it.
pub fn check(
    pending: Option<&PendingClaim>,
    has_members: bool,
    wallet: Principal,
    caller: Principal,
    now_ns: u64,
    signature: &[u8],
) -> Result<(), WalletError> {
    let Some(pending) = pending else {
        return Err(WalletError::AlreadyClaimed);
    };
    if has_members {
        return Err(WalletError::AlreadyClaimed);
    }
    // Anyone can call as the anonymous principal, so it can never own.
    if caller == Principal::anonymous() {
        return Err(WalletError::Forbidden(
            "the anonymous principal cannot claim".to_string(),
        ));
    }
    if now_ns >= pending.expires_at_ns {
        return Err(WalletError::ClaimExpired);
    }
    let message = message(wallet, caller);
    if !ed25519::verify(&pending.public_key, message.as_bytes(), signature) {
        return Err(WalletError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_bytes::ByteBuf;

    const NOW: u64 = 1_000;

    fn claim_key() -> SigningKey {
        SigningKey::from_bytes(&[5; 32])
    }

    fn wallet() -> Principal {
        Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 2, 1, 1])
    }

    fn user(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn pending() -> PendingClaim {
        PendingClaim {
            public_key: ByteBuf::from(claim_key().verifying_key().to_bytes().to_vec()),
            expires_at_ns: NOW + 1,
        }
    }

    fn sign(wallet: Principal, caller: Principal) -> Vec<u8> {
        let message = message(wallet, caller);
        claim_key().sign(message.as_bytes()).to_bytes().to_vec()
    }

    fn check_now(caller: Principal, signature: &[u8]) -> Result<(), WalletError> {
        check(Some(&pending()), false, wallet(), caller, NOW, signature)
    }

    #[test]
    fn the_message_names_the_wallet_and_the_caller() {
        assert_eq!(
            message(wallet(), Principal::anonymous()),
            format!("claim:{}:2vxsx-fae", wallet().to_text())
        );
    }

    #[test]
    fn accepts_the_signature_for_the_caller() {
        assert_eq!(check_now(user(1), &sign(wallet(), user(1))), Ok(()));
    }

    #[test]
    fn refuses_a_signature_for_another_principal() {
        let signature = sign(wallet(), user(1));
        assert_eq!(
            check_now(user(2), &signature),
            Err(WalletError::BadSignature)
        );
    }

    #[test]
    fn refuses_a_signature_for_another_wallet() {
        let other = Principal::from_slice(&[0, 0, 0, 0, 0, 0, 0, 3, 1, 1]);
        let signature = sign(other, user(1));
        assert_eq!(
            check_now(user(1), &signature),
            Err(WalletError::BadSignature)
        );
    }

    #[test]
    fn refuses_a_signature_by_another_key() {
        let message = message(wallet(), user(1));
        let signature = SigningKey::from_bytes(&[6; 32]).sign(message.as_bytes());
        assert_eq!(
            check_now(user(1), &signature.to_bytes()),
            Err(WalletError::BadSignature)
        );
    }

    #[test]
    fn refuses_the_anonymous_principal_even_with_its_signature() {
        let anonymous = Principal::anonymous();
        assert!(matches!(
            check_now(anonymous, &sign(wallet(), anonymous)),
            Err(WalletError::Forbidden(_))
        ));
    }

    #[test]
    fn refuses_an_expired_claim_from_its_expiry_on() {
        let signature = sign(wallet(), user(1));
        for now in [NOW + 1, NOW + 2] {
            assert_eq!(
                check(Some(&pending()), false, wallet(), user(1), now, &signature),
                Err(WalletError::ClaimExpired)
            );
        }
    }

    #[test]
    fn refuses_once_claimed() {
        let signature = sign(wallet(), user(1));
        assert_eq!(
            check(None, false, wallet(), user(1), NOW, &signature),
            Err(WalletError::AlreadyClaimed)
        );
        assert_eq!(
            check(Some(&pending()), true, wallet(), user(1), NOW, &signature),
            Err(WalletError::AlreadyClaimed)
        );
    }
}
