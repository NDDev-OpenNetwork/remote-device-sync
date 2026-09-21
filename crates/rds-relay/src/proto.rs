//! Owned relay protocol — design scaffold.
//!
//! The relay forwards opaque datagrams between endpoints. Because QUIC
//! is end-to-end encrypted, the relay is a dumb packet mover: it sees
//! `EndpointId`s and byte counts, never content.
//!
//! ```text
//! client ──QUIC──▶ relay
//!   REGISTER { endpoint_id }            (mutually authed QUIC conn)
//!   FORWARD  { dst_endpoint_id, bytes } (unreliable datagram channel)
//!   relay → dst's connection            FORWARD { src_endpoint_id, bytes }
//! ```
//!
//! Endpoints attach to their assigned relay (GDS registry picks it),
//! the relay routes datagrams between attached endpoints, and the
//! endpoint's noq socket mux surfaces them as an ordinary network path.
//! The same channel carries QNT `REACH_OUT`/`PUNCH_ME` coordination —
//! hole punching works *through* the relay path like on iroh.
//!
//! Server duties beyond forwarding: presence (who is attached),
//! per-endpoint rate accounting, drain/failover signalling, and the
//! health/metrics surface the GDS controller scrapes.
//!
//! Wire format: postcard-framed control messages on a bidi stream +
//! raw datagrams on the QUIC DATAGRAM channel for payload. Envelope
//! versioning rides the ALPN (`rds-relay/0`).

/// ALPN for the owned relay protocol.
pub const RELAY_ALPN: &[u8] = b"rds-relay/0";
