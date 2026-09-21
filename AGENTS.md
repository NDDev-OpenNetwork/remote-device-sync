# remote-device-sync — agent instructions

Public Rust module of the NDDev-OpenNetwork estate. The product is remote
device access and sync: SSH connectivity, remote desktop sessions and file
synchronization between devices, brokered through the GDS server.

## What this module is

A Cargo workspace under `crates/` — see `docs/architecture.md` for the
crate map and dependency direction. Estate, tenant, host and credential
facts do not belong here — the module is public and must stay free of
private data.

## Before changing anything

- `docs/architecture.md` — crate map, protocol decisions, identity model
- `docs/research.md` — deep research incl. the full-ownership plan (§9)
- `docs/platforms.md` — supported OSes (Linux x86_64, macOS arm64),
  backend probe orders, cfg conventions
- `docs/conventions.md` — layering, safety, errors, wire, testing rules
- `docs/roadmap.md` — milestone gates; a change belongs to a milestone

## Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features rds-desktop/x11 -- -D warnings   # linux lane
cargo test --workspace
cargo deny check   # if cargo-deny installed
```

CI runs fmt + clippy + test on `ubuntu-latest` and `macos-latest`.

## Git

Conventional Commits, signed commits, `main` is pull-request only.
