//! Owned transport backend on `noq` — work in progress.
//!
//! Design (docs/research.md §9.1): noq is the QUIC engine; everything
//! above it is ours:
//!
//! - `Endpoint::new_with_abstract_socket(Box<dyn AsyncUdpSocket>)` is
//!   the seam — our socket muxes direct UDP, relay tunnels and multiple
//!   interfaces behind one virtual socket.
//! - `Connection::open_path` turns discovered candidate addresses into
//!   QUIC paths; `PathEvents`/`NatTraversalUpdates` report QNT progress.
//! - `observed_external_addr` gives us our public address in-band —
//!   no STUN service required.
//! - Path policy (`PathSelector`-equivalent at our layer) pins
//!   control/input traffic to the most stable path while media rides
//!   the lowest-RTT one.
//!
//! Components to land here:
//!
//! - `socket::Mux` — `AsyncUdpSocket` impl over real UDP + relay link.
//! - `endpoint` — bind/connect/accept wrapper exposing our
//!   `EndpointId`-keyed API (same shape as the iroh facade).
//! - `policy` — path-open order, per-service path pinning, probing.
//! - `relay_link` — client side of the rds relay protocol
//!   (`rds-relay::proto`).
//!
//! Until this backend reaches parity (same-harness benchmarks vs the
//! iroh backend), `iroh` remains the default selected at bind time.
