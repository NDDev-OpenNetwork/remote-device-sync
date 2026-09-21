# Checkpoint C2 — 2026-09-21

Verdict: **PASS with one deferral** — the owned relay carries the full
QUIC handshake, datagrams and streams end-to-end; replacement, drain and
rate limiting are proven. Multi-relay failover is deferred (needs
multi-relay endpoint config).

## Automated checks

- `cargo fmt --check`: PASS
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `cargo clippy --workspace --all-targets --features transport-noq + owned-relay -- -D warnings`: PASS
- `cargo test --workspace`: PASS
- `cargo test -p rds-net -p rds-agent -p rds-relay --features transport-noq + owned-relay`: PASS
  - noq backend: 5 tests (loopback, datagrams, iroh↔noq both ways, mux second-socket)
  - agent e2e: 4 tests incl. `Backend::Noq` variant
  - turmoil partition/repair: PASS (regression lock held)
  - relay unit: 2 bucket tests; owned e2e: 3 tests
- `rds-core` relay decoder proptest: PASS (well-formed roundtrips,
  arbitrary bytes never panic)

## What the wave built

- `rds_core::relay`: shared wire protocol — `RELAY_ALPN`, control
  messages (Register/Registered/PeerGone/Ping/Pong/Drain/Health),
  `[32B key][payload]` forward frames, bounds-checked decoder.
- `rds_relay::server`: owned relay — noq endpoint on `rds-relay/0`,
  per-endpoint slots keyed by TLS-verified `EndpointId`, datagram
  forward loop, per-source token bucket (64 MiB/s, 4 MiB burst),
  `PeerGone` fan-out, drain broadcast + close, replacement on
  re-register (stale-slot detach is identity-checked).
- `rds_net::backends::noq::relay`: `RelaySocket` mux child — helper
  endpoint (same key → same identity) registers over the control bidi,
  datagram pump maps `src_key → synthetic_addr` in `198.19.0.0/16`,
  `RelayHandle` for peer registration + drain state. Synthetic
  destinations route through the tunnel in `MuxSender::pick`; synthetic
  locals are never advertised as direct candidates.
- `EndpointConfig::relay_endpoint`: typed owned-relay attachment;
  `Endpoint::addr()` advertises `TransportAddr::Relay(rds-relay://id@host)`.

## e2e evidence

`relay_forwards_handshake_and_datagrams`: two noq endpoints on one
relay, target reduced to its relay candidate — handshake, datagrams
both directions, bidi streams, relay stats (forwarded>0, dropped=0).
`reattach_replaces_stale_slot`: same-key re-attach leaves exactly one
slot. `drain_evicts_and_refuses_new_attachments`: drain empties the
table and new attach fails.

## Bugs found and fixed by the gate

- Tunnel MTU: outer packets >~1408B exceeded the helper conn's datagram
  budget → `TooLarge` socket error killed the path. Relay sender now
  drops oversized transmits like a link with a smaller MTU; path-MTU
  discovery converges below the ceiling.
- Stale-detach eviction: a replaced connection's cleanup removed the
  fresh slot by id. `detach` now only removes if the stored slot is the
  same `Arc`.
- Dead-child poisoning: a stopped relay pump returned `BrokenPipe`
  through the mux, which would kill the endpoint. Dead transport now
  surfaces nothing (link-down semantics).
- Test deadlock: server-side handshake only advances once `accept()` is
  polled; e2e accepts concurrently.
- postcard `use-std` was only enabled transitively; now explicit in the
  three crates using `to_stdvec`.

## Not run

- relay-kill → second-relay migration: endpoint config carries a single
  `relay_endpoint`; multi-relay attach lands with the discovery wave
  (WS3) which owns relay set selection.
- Relay-in-turmoil sim: `RelaySocket` binds a real helper socket; a
  sim-socket injection point is needed first (deferred with WS2 follow-up).

## Manual checklist

- [x] All "not run" items above explained
- [x] Reports committed: this file
- [x] docs/ updated — `docs/implementation-plan.md` covers WS2 scope
- [x] security/unsafe review — no new `unsafe`; relay sees only
  endpoint ids + byte counts (payload is end-to-end encrypted QUIC)
