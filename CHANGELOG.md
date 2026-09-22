# Changelog

## [Unreleased]

- Post-WS3 consistency + hardening: `rds-desktop` now programs
  against the `rds_net` facade (`Connection` plus the stream/error
  re-exports — the documented "+rds-net when streams abstract"
  direction), so the `desktop` feature on `rds-agent`/`rds-cli`
  compiles and serves on both transport backends instead of failing
  on a raw-`iroh` type mismatch; a new CI lane
  `--features rds-agent/desktop,rds-cli/desktop` (both OSes) keeps it
  compiling. Session teardown fixed: `serve_desktop` aborts the
  frame-writer task and the capture producer exits when its channel
  closes — `JoinHandle::abort` cannot interrupt `spawn_blocking`
  work, so ending a session previously leaked a permanently-spinning
  capture/encode thread; `DesktopSession` drop aborts the
  frame-receiver task so the connection closes with the session
  instead of outliving it. The directory's global per-minute window
  now covers every signature-verifying write — `PUT /v1/records`,
  `PUT /v1/registry`, `DELETE /v1/records/{key}` — checked before
  parsing, exported as `rds_directory_writes_rate_limited`.
  `x11rb` 0.13 → 0.14.
- WS3 discovery service + publish/resolve (`rds-discovery`):
  `EndpointRecord` payloads carry `issued_at`; stores reject
  forged, expired and same-age-or-older replayed records, and honor
  signed `DeleteRequest` tombstones with replay protection. Minimal
  owned HTTP/1.1 codec (`http.rs` — hard head/body caps, one request
  per connection) serving `PUT/GET/DELETE /v1/records`,
  `GET /v1/names/{name}`, `PUT /v1/registry`, `/v1/health`,
  `/v1/metrics` with per-key + global PUT rate limiting
  (`service.rs`), plus a matching timeout-bounded `client.rs`.
  Estate name→key resolution rides a signed `SignedRegistry`
  snapshot (`registry.rs`) verified against a configured registry
  key — the directory can withhold but never invent names.
  `rds-net`: `announce` task publishes on start, refreshes at
  `ttl/3`, and republishes promptly when the advertised address set
  changes; `resolve_target` resolves ticket → bare key → device
  name. `rds` CLI gains `--server`; `rds-agent` gains `--directory` /
  `--record-ttl`; `rds-server` composes relay + directory + registry
  flags. `rds-bench` gains the G3 `resolve-connect` scenario: cold
  name→resolve→connect→first-byte per fresh endpoint. Gate `c3`:
  G3 p50 well under the 300 ms budget (see
  `docs/reports/checkpoint-c3.md`).
- WS2 owned relay transport (`rds-relay` + `rds-net::backends::noq::relay`):
  shared wire protocol in `rds_core::relay` (`rds-relay/0` ALPN, control
  stream, `[32B key][payload]` datagram frames); owned server with
  per-endpoint slots keyed by TLS-verified identity, token-bucket rate
  limiting, `PeerGone` fan-out, drain broadcast, identity-checked
  replacement. Client side: `RelaySocket` joins the socket mux as a
  tunnel child — a same-key helper endpoint registers on the control
  bidi, peers map to synthetic `198.19.0.0/16` addresses, and
  `EndpointConfig::relay_endpoint` attaches it. e2e proves handshake +
  datagrams + streams entirely through the tunnel, stale-slot
  replacement, and drain eviction. Gate `c2` passed; multi-relay
  failover deferred to WS3.
- WS1c connection driver + path migration: per-connection
  `connection_driver` task (`policy.rs`) consumes QNT address
  advertisements (`ADD_ADDRESS`/`REMOVE_ADDRESS`) and path events, and
  applies a biased-RTT path selection — the lowest-RTT path becomes
  `Available`, all others `Backup`, with a 5ms stickiness margin to
  prevent flapping. Holds only `WeakConnectionHandle`/`WeakPathHandle`
  so the task never keeps a connection alive (last-handle-drop stays the
  close trigger). Both endpoints advertise their real socket addrs and
  the client initiates traversal rounds, so peers can upgrade paths
  learned in-band. Result: under the `impaired` scenario (5% loss,
  50+30ms delay via proxy) noq now migrates traffic to a direct path —
  p50 1.80ms vs 136.5ms before the driver, at parity with iroh's 1.68ms.
  Deterministic `turmoil` partition/repair simulation stays green; the
  socket-mux accepts handshakes on non-primary sockets.
- WS1b backend-neutral `rds-net` facade: unified `EndpointConfig`
  (backend, secret key, bind addr, relay, ALPNs) and `Backend` selector;
  `Endpoint`/`Connection`/`Incoming` wrappers dispatch to either
  substrate while sharing the same `noq` stream/error types.
  `rds-agent`, `rds-cli` and `rds-bench` no longer depend on `iroh`
  directly — `--backend iroh|noq` selects at bind time. Owned-backend
  `addr()` now advertises dialable candidates (loopback + kernel egress
  hint) instead of the unspecified bind address. New evidence: agent
  e2e runs on `Backend::Noq`, and `rds-bench --backend noq` produces
  same-harness parity numbers on the direct path (handshake p50 20.5ms
  vs iroh 22.6ms; ping p50 1.31ms vs 1.49ms; suite compare within
  tolerance). Relay paths correctly refuse on `noq` until WS2.
- WS1 owned transport backend (`rds-net::backends::noq`, behind the
  `transport-noq` feature): noq endpoint with our own RFC 7250
  raw-public-key TLS layer (`tls.rs` — Ed25519 SPKI certs, server-name
  `EndpointId` verification, mutual client certs) that is
  wire-compatible with the shipping iroh backend on `rds/0`; BLAKE3
  stateless-reset key; endpoint facade (`bind/connect/accept/addr/close`)
  mirroring the iroh surface; candidate pipeline (`policy.rs` — dedup,
  cap at multipath limit, `open_path_ensure` for extra candidates, QNT
  round trigger); transport config matching iroh defaults (multipath ×8,
  QNT addrs ×32, QUIC datagrams, path keepalive/idle). Integration tests
  prove noq↔noq loopback, QUIC datagrams, and cross-backend interop
  (iroh client ↔ noq server, both directions). `iroh` remains the
  default backend until C1 parity.
- WS0 measurement harness: new dev-only `rds-bench` crate — scenario
  runner (handshake/ping/transfer/multiconnect/relay-fallback/impaired)
  on real QUIC paths, deterministic seeded UDP impairment proxy
  (loss/delay/jitter/rate), JSON+markdown reports and suite-vs-suite
  drift comparison (`max(rel, floor)` rule, separate tail band).
  `scripts/checkpoint.sh` implements the gate machinery from
  `docs/implementation-plan.md`; gate `c0` passed with committed
  evidence under `docs/reports/` (baseline-iroh.md).
- Workspace restructure for the full-ownership architecture
  (`docs/research.md` §9): `rds-transport` renamed `rds-net` with
  `backends::{iroh,noq}`; new crates `rds-discovery` (signed endpoint
  records, memory/file stores), `rds-sync` (FastCDC+BLAKE3 manifests,
  delta computation), `rds-audio` (Opus pipeline scaffold),
  `rds-server` (GDS services host: relay + discovery directory);
  `rds-relay` gains a lib + `proto` owned-protocol scaffold;
  `rds-desktop` reorganized into `capture`/`codec`/`input`/`render`
  backend dirs with per-platform probe order.
- `docs/platforms.md`, `docs/conventions.md`, `docs/roadmap.md`:
  platform matrix (Linux x86_64, macOS arm64), engineering rules,
  milestone gates. CI matrix on both OSes; `rust-toolchain.toml`,
  `deny.toml`, workspace lint floor.
- Core transport foundation: six-crate workspace (`rds-core`,
  `rds-transport`, `rds-relay`, `rds-agent`, `rds-cli`, `rds-desktop`) on
  iroh 1.2 / QUIC with Ed25519 endpoint identity, hole-punched direct
  paths and self-hosted relay fallback.
- `rds` CLI: `id`, `ticket`, `ping`, `info`, `ssh`, `forward`, `desktop`.
- `rds-agent`: peer `EndpointId` allowlist, scoped TCP forwarding
  (default: local sshd only), desktop session service behind `desktop`.
- `rds-relay`: embedded iroh-relay with endpoint allowlist.
- `rds-desktop`: capturer/encoder/decoder/input traits, MoQ-style
  per-frame uni streams with stale-frame drop, X11 capture + XTEST input
  + OpenH264 codec behind the `x11` feature.
- `docs/architecture.md`: transport/desktop research and decisions.
- Initial Rust skeleton: library crate and `remote-device-sync` CLI stub.
