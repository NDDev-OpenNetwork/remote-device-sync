//! Network layer for rds: endpoint lifecycle, persistent identity,
//! ticket encoding, and transport backends.
//!
//! The network identity of an rds peer is an Ed25519 key pair; QUIC-TLS
//! authentication is derived from it, so a connection is always pinned
//! to a key rather than an IP address.
//!
//! # Backends
//!
//! Two backends live behind this crate's facade:
//!
//! - [`backends::iroh`] — the shipping substrate: iroh endpoint with
//!   hole punching, relay fallback and n0/pkarr discovery plumbing.
//! - [`backends::noq`] — the owned transport under construction: a noq
//!   endpoint with our own socket mux, path policy, relay transport and
//!   GDS discovery. See `docs/research.md` §9 for the design.
//!
//! Dependent crates program against the re-exported API surface; the
//! concrete backend is selected at `bind_endpoint` time (iroh today).

pub mod backends {
    /// Current transport substrate (iroh 1.x on noq underneath).
    pub mod iroh;
    /// Owned transport: noq endpoint + our socket/path/discovery layer.
    #[cfg(feature = "transport-noq")]
    pub mod noq;
}

pub use backends::iroh::*;
