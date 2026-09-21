# Changelog

## [Unreleased]

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
