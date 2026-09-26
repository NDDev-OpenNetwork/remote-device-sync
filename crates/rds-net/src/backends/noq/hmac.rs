//! Stateless-reset key for the QUIC endpoint.
//!
//! `EndpointConfig` requires an [`noq::crypto::HmacKey`] to authenticate
//! stateless resets sent to peers that were talking to a previous instance
//! of this endpoint. We use a keyed BLAKE3 MAC: it is already a workspace
//! dependency and needs no extra crypto provider wiring.

/// QUIC reset-token key backed by keyed BLAKE3.
pub struct Blake3HmacKey([u8; 32]);

impl std::fmt::Debug for Blake3HmacKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Blake3HmacKey([REDACTED])")
    }
}

impl Blake3HmacKey {
    /// Generate a fresh reset key from a cryptographically secure RNG.
    pub fn new(rng: &mut impl rand::CryptoRng) -> Self {
        let mut key = [0u8; 32];
        rng.fill_bytes(&mut key);
        Self(key)
    }
}

impl noq::crypto::HmacKey for Blake3HmacKey {
    fn sign(&self, data: &[u8], signature_out: &mut [u8]) {
        signature_out.copy_from_slice(blake3::keyed_hash(&self.0, data).as_slice());
    }

    fn signature_len(&self) -> usize {
        blake3::OUT_LEN
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<(), noq::crypto::CryptoError> {
        let expected = blake3::keyed_hash(&self.0, data);
        // Hash's comparison is constant-time for a correctly sized tag;
        // converting it to a byte slice would lose that property.
        if expected.eq(signature) {
            Ok(())
        } else {
            Err(noq::crypto::CryptoError {})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noq::crypto::HmacKey;

    #[test]
    fn reset_key_debug_redacts_secret() {
        let key = Blake3HmacKey([0x42; 32]);
        assert_eq!(format!("{key:?}"), "Blake3HmacKey([REDACTED])");
    }

    #[test]
    fn reset_mac_binds_key_message_and_tag_length() {
        let key = Blake3HmacKey::new(&mut rand::rng());
        let mut signature = vec![0; key.signature_len()];
        key.sign(b"connection-id", &mut signature);
        assert!(key.verify(b"connection-id", &signature).is_ok());
        assert!(key.verify(b"another-id", &signature).is_err());
        assert!(
            Blake3HmacKey::new(&mut rand::rng())
                .verify(b"connection-id", &signature)
                .is_err()
        );
        assert!(key.verify(b"connection-id", &signature[..31]).is_err());
        signature.push(0);
        assert!(key.verify(b"connection-id", &signature).is_err());
        signature.pop();
        signature[0] ^= 1;
        assert!(key.verify(b"connection-id", &signature).is_err());
    }
}
