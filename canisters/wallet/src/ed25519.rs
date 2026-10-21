//! Ed25519 checks for the claim key. Release signer keys are the `release`
//! crate's.

use ed25519_dalek::{Signature, VerifyingKey};

/// Decodes a public key, refusing bytes that are not a curve point and the
/// weak keys of small order, for which a signature proves nothing.
pub fn public_key(bytes: &[u8]) -> Option<VerifyingKey> {
    let key = VerifyingKey::from_bytes(bytes.try_into().ok()?).ok()?;
    (!key.is_weak()).then_some(key)
}

/// Strict verification: canonical encodings only, and no small-order key or
/// signature point, so one signature cannot be reshaped into another.
pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Some(key) = self::public_key(public_key) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify_strict(message, &signature).is_ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[4; 32])
    }

    #[test]
    fn accepts_a_signature_by_the_key() {
        let key = signing_key();
        let signature = key.sign(b"message").to_bytes();
        assert!(verify(
            key.verifying_key().as_bytes(),
            b"message",
            &signature
        ));
    }

    #[test]
    fn refuses_another_message_or_a_changed_signature() {
        let key = signing_key();
        let mut signature = key.sign(b"message").to_bytes();
        assert!(!verify(
            key.verifying_key().as_bytes(),
            b"other",
            &signature
        ));
        signature[0] ^= 1;
        assert!(!verify(
            key.verifying_key().as_bytes(),
            b"message",
            &signature
        ));
        assert!(!verify(
            key.verifying_key().as_bytes(),
            b"message",
            &signature[..63]
        ));
    }

    #[test]
    fn refuses_a_small_order_key() {
        // The identity point: every signature with R = identity and s = 0
        // would verify under a non-strict check.
        let mut identity = [0; 32];
        identity[0] = 1;
        assert!(public_key(&identity).is_none());
        let mut forged = [0; 64];
        forged[0] = 1;
        assert!(!verify(&identity, b"message", &forged));
    }

    #[test]
    fn refuses_bytes_that_are_not_a_key() {
        assert!(public_key(&[4; 31]).is_none());
        assert!(public_key(&[4; 33]).is_none());
        assert!(public_key(&NOT_A_POINT).is_none());
    }

    /// 32 bytes whose y coordinate has no point on the curve.
    pub(crate) const NOT_A_POINT: [u8; 32] = [2; 32];
}
