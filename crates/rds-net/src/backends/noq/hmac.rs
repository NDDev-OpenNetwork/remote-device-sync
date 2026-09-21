//! Stateless-reset key for the QUIC endpoint.
//!
//! `EndpointConfig` requires an [`noq::crypto::HmacKey`] to authenticate
//! stateless resets sent to peers that were talking to a previous instance
//! of this endpoint. We use a keyed BLAKE3 MAC: it is already a workspace
//! dependency and needs no extra crypto provider wiring.

/// HMAC-SHA-256-equivalent reset key backed by keyed BLAKE3.
#[derive(Debug)]
pub struct Blake3HmacKey([u8; 32]);

impl Blake3HmacKey {
    /// Generate a fresh random reset key.
    pub fn new(rng: &mut impl rand::RngCore) -> Self {
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
        if signature == expected.as_slice() {
            Ok(())
        } else {
            Err(noq::crypto::CryptoError {})
        }
    }
}
