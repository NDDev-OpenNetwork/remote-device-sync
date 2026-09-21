//! Owned relay protocol — wire types (ALPN `rds-relay/0`).
//!
//! The relay forwards opaque datagrams between endpoints. Because QUIC
//! is end-to-end encrypted, the relay is a dumb packet mover: it sees
//! endpoint keys and byte counts, never content.
//!
//! ```text
//! client ──QUIC(rds-relay/0)──▶ relay
//!   control bidi stream: length-prefixed postcard RelayControl
//!   payload: QUIC datagrams [32B dst_key][payload]  client → relay
//!                          [32B src_key][payload]  relay  → client
//! ```
//!
//! Endpoints attach to their assigned relay, the relay routes datagrams
//! between attached endpoints, and the endpoint's socket mux surfaces
//! them as an ordinary network path. The same channel carries hole-punch
//! coordination traffic — it is just more datagrams to move.
//!
//! Server duties beyond forwarding: presence (who is attached),
//! per-endpoint rate accounting, drain signalling, and the health
//! surface the GDS controller scrapes.
//!
//! Endpoint keys are the raw 32-byte Ed25519 public key — the same key
//! material `EndpointId` wraps — so this module stays dependency-free.

use serde::{Deserialize, Serialize};

/// ALPN for the owned relay protocol.
pub const RELAY_ALPN: &[u8] = b"rds-relay/0";

/// Bytes in a forwarded datagram consumed by the endpoint key header.
pub const KEY_HEADER_LEN: usize = 32;

/// Maximum payload a forwarded datagram may carry.
///
/// The relay refuses larger frames outright; clients keep payloads under
/// the QUIC path MTU anyway (forwarded bytes are outer-QUIC packets of
/// the tunnelled connection).
pub const MAX_PAYLOAD: usize = 64 * 1024;

/// An endpoint's 32-byte Ed25519 public key (`EndpointId` material).
pub type Key = [u8; 32];

/// Control messages on the per-connection bidirectional stream.
///
/// Postcard-framed with a `u32` length prefix; versioning rides the
/// ALPN, so a wire change means a new ALPN, not an in-band version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelayControl {
    /// Client → server: attach this authenticated connection for
    /// forwarding. The endpoint identity comes from TLS peer auth —
    /// the frame carries no key material to spoof.
    Register,
    /// Server → client: registration accepted; datagrams may flow.
    Registered,
    /// Server → clients that recently talked to `peer`: it detached.
    /// Datagrams addressed to it are dropped; senders should let the
    /// path die.
    PeerGone { peer: Key },
    /// Bidirectional liveness probes.
    Ping { seq: u64 },
    /// Reply to [`RelayControl::Ping`].
    Pong { seq: u64 },
    /// Server → all clients: the relay is draining. Clients should
    /// migrate flows off relayed paths and re-register elsewhere; the
    /// server stops accepting new registrations.
    Drain,
    /// Server → client: periodic stats snapshot.
    Health {
        /// Attached endpoints.
        endpoints: u64,
        /// Whether the relay is draining.
        draining: bool,
    },
}

/// Encode a client→relay datagram: `[dst_key][payload]`.
pub fn encode_forward(dst: &Key, payload: &[u8]) -> bytes::Bytes {
    let mut out = bytes::BytesMut::with_capacity(KEY_HEADER_LEN + payload.len());
    out.extend_from_slice(dst);
    out.extend_from_slice(payload);
    out.freeze()
}

/// Decode a datagram header: `(key, payload)`.
///
/// Returns `None` for malformed frames — undersized or oversized —
/// which callers must drop, never forward.
pub fn decode_frame(frame: &[u8]) -> Option<(Key, &[u8])> {
    if frame.len() < KEY_HEADER_LEN || frame.len() > KEY_HEADER_LEN + MAX_PAYLOAD {
        return None;
    }
    let (key, payload) = frame.split_at(KEY_HEADER_LEN);
    let mut raw = [0u8; 32];
    raw.copy_from_slice(key);
    Some((raw, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_roundtrip() {
        let dst = [7u8; 32];
        let frame = encode_forward(&dst, b"quic-packet-bytes");
        let (id, payload) = decode_frame(&frame).unwrap();
        assert_eq!(id, dst);
        assert_eq!(payload, b"quic-packet-bytes");
    }

    #[test]
    fn malformed_frames_rejected() {
        assert!(decode_frame(&[]).is_none());
        assert!(decode_frame(&[0u8; 31]).is_none());
        // Just the header, empty payload — legal.
        let (id, payload) = decode_frame(&[7u8; 32]).unwrap();
        assert_eq!(payload.len(), 0);
        let _ = id;
        // Oversized.
        assert!(decode_frame(&vec![0u8; KEY_HEADER_LEN + MAX_PAYLOAD + 1]).is_none());
    }

    #[test]
    fn control_postcard_roundtrip() {
        for msg in [
            RelayControl::Register,
            RelayControl::Registered,
            RelayControl::PeerGone { peer: [9u8; 32] },
            RelayControl::Ping { seq: 7 },
            RelayControl::Pong { seq: 7 },
            RelayControl::Drain,
            RelayControl::Health {
                endpoints: 3,
                draining: true,
            },
        ] {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            let back: RelayControl = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(format!("{back:?}"), format!("{msg:?}"));
        }
    }

    proptest::proptest! {
        #[test]
        fn decode_never_panics_and_roundtrips(
            key in proptest::array::uniform32(proptest::num::u8::ANY),
            payload in proptest::collection::vec(proptest::num::u8::ANY, 0..=MAX_PAYLOAD),
            garbage in proptest::collection::vec(proptest::num::u8::ANY, 0..=KEY_HEADER_LEN + MAX_PAYLOAD + 64),
        ) {
            // Well-formed frames always decode.
            let frame = encode_forward(&key, &payload);
            let (id, body) = decode_frame(&frame).unwrap();
            proptest::prop_assert_eq!(id, key);
            proptest::prop_assert_eq!(body, &payload[..]);

            // Arbitrary bytes never panic; they either decode to a
            // 32-byte header plus payload or are rejected.
            if let Some((_, body)) = decode_frame(&garbage) {
                proptest::prop_assert!(garbage.len() >= KEY_HEADER_LEN);
                proptest::prop_assert!(garbage.len() <= KEY_HEADER_LEN + MAX_PAYLOAD);
                proptest::prop_assert_eq!(body.len() + KEY_HEADER_LEN, garbage.len());
            }
        }
    }
}
