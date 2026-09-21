# remote-device-sync — agent instructions

Public Rust module of the NDDev-OpenNetwork estate. The product is remote
device access: SSH connectivity and remote desktop sessions between devices,
brokered through the GDS server. The protocol design is not settled yet.

## What this module is

A library crate (`src/lib.rs`) with a thin CLI binary (`src/main.rs`). Estate,
tenant, host and credential facts do not belong here — the module is public
and must stay free of private data.

## Checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Git

Conventional Commits, signed commits, `main` is pull-request only.
