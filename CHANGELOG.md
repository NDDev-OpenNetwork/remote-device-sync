# Changelog

## [Unreleased]

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
