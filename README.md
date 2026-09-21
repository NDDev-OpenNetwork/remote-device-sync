# remote-device-sync

Remote device access for the GDS estate: connect from any enrolled device to
another over SSH, with remote desktop sessions brokered through the GDS
server. The transport and session contracts are still being designed — this
repository currently carries the Rust skeleton.

## Status

Early design. Public API, wire protocol and RDP integration are not stable.

## Build and test

Requires a stable Rust toolchain (edition 2024).

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
