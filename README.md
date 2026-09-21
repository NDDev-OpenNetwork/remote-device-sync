# remote-device-sync

Remote device access for the GDS estate: connect from any enrolled device to
another over SSH, with remote desktop sessions — transport is QUIC via
[iroh](https://crates.io/crates/iroh), so peers are Ed25519 keys, direct
paths are hole-punched, and a self-hosted relay covers egress-only
networks. See [docs/architecture.md](docs/architecture.md) for the research
and protocol decisions.

## Layout

```text
crates/
├── rds-core       wire protocol: ALPN rds/0, postcard frames, service types
├── rds-transport  iroh endpoint lifecycle, key persistence, tickets
├── rds-relay      self-hostable relay binary (embedded iroh-relay + allowlist)
├── rds-agent      daemon on a controlled device (ssh forward, desktop serve)
├── rds-cli        `rds` operator CLI
└── rds-desktop    capture/encode/input/decode traits + X11/OpenH264 impls
```

## Usage

```sh
# On the controlled device: run the agent (key persisted under ~/.config/rds)
rds-agent --allow <peer-endpoint-id> --ssh 127.0.0.1:22

# From the operator device, using the agent's printed ticket:
rds ticket                     # your own endpoint ticket
rds ping <ticket>              # RTT probes
rds ssh <ticket> -L 127.0.0.1:2222
ssh -p 2222 user@127.0.0.1     # reaches the remote sshd over QUIC
rds forward <ticket> -L 127.0.0.1:8080 --remote 127.0.0.1:3000
rds desktop <ticket>           # requires --features desktop on both ends

# Self-hosted relay instead of the public n0 relays:
rds-relay --bind 0.0.0.0:3340
rds-agent --relay http://relay.host:3340 ...
rds --relay http://relay.host:3340 ...
```

Every hop is end-to-end encrypted between the two endpoints; the relay sees
only ciphertext. The agent rejects any peer not on its `--allow` list before
a service stream opens.

## Build and test

Requires a stable Rust toolchain (edition 2024).

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

The `desktop` feature (`rds-agent`, `rds-cli`) and `x11` feature
(`rds-desktop`) enable capture/input/codec code; they build on any Linux
host but need a live X11 display to function. Wayland portal/PipeWire,
DXGI and ScreenCaptureKit backends are roadmap items — see the milestones
in the architecture doc.

## Status

v0.1 foundation: working SSH/TCP forwarding over direct and relayed QUIC,
endpoint allowlists, desktop pipeline skeleton behind feature gates.
Public API and wire protocol are not stable.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
