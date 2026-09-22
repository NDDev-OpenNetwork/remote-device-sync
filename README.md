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
├── rds-net        endpoint lifecycle, keys, tickets — backends::iroh
│                  (shipping) + backends::noq (owned transport, §9)
├── rds-discovery  signed endpoint records + stores for GDS discovery
├── rds-relay      relay server lib+bin; proto = owned relay protocol
├── rds-server     GDS services host: relay + discovery + registry
├── rds-agent      daemon on a controlled device (ssh forward, desktop)
├── rds-cli        `rds` operator CLI
├── rds-desktop    capture/codec/input/render traits + platform backends
├── rds-audio      Opus audio pipeline (scaffold)
└── rds-sync       FastCDC+BLAKE3 content-addressed sync
```

Docs: [architecture](docs/architecture.md) · [deep research](docs/research.md)
· [platforms](docs/platforms.md) · [conventions](docs/conventions.md) ·
[roadmap](docs/roadmap.md).

## Usage

```sh
# On the controlled device: run the agent (key persisted under ~/.config/rds)
rds-agent --allow <peer-endpoint-id> --ssh 127.0.0.1:22

# From the operator device, using the agent's printed ticket:
rds id                         # your bare endpoint id (hex) for --allow lists
rds ticket                     # your own endpoint ticket
rds ping <ticket>              # RTT probes + per-path stats
rds ssh <ticket> -L 127.0.0.1:2222
ssh -p 2222 user@127.0.0.1     # reaches the remote sshd over QUIC
rds forward <ticket> -L 127.0.0.1:8080 --remote 127.0.0.1:3000
rds send <ticket> <file>       # resumable content-addressed push (agent --sync-dir)
rds recv <ticket> <name>       # pull a file back out of the sync dir
rds desktop <ticket>           # requires --features desktop on both ends

# Self-hosted relay instead of the public n0 relays:
rds-relay --bind 0.0.0.0:3340
rds-agent --relay http://relay.host:3340 ...
rds --relay http://relay.host:3340 ...

# rds-server composes relay + signed discovery directory on one host
# (see docs/deployment.md for the systemd units and firewall rules):
rds-server --relay-addr 0.0.0.0:3340 --http-addr 0.0.0.0:3341 \
    --directory /var/lib/rds/directory --allow <endpoint-id>...
```

Every hop is end-to-end encrypted between the two endpoints; the relay sees
only ciphertext. The agent rejects any peer not on its `--allow` list before
a service stream opens, and can additionally require estate-signed capability
grants (`--issuer`, scope-checked per stream, revocable via the directory's
`/v1/revocations` feed).

Observability: `rds-net` exposes a per-endpoint metrics registry (per-path
RTT/loss/cwnd, datagrams and bytes split by `via="direct"`/`via="relay"`,
QNT counters on the owned transport) with Prometheus text export behind the
`metrics` feature; the directory serves it at `GET /v1/metrics` (loopback
peers only) with per-writer counters under anonymized labels, and every agent
connection logs inside an `rds.conn{peer, session_id}` span.

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

v0.1 foundation: SSH/TCP forwarding over direct and relayed QUIC,
estate-signed capability grants with revocation, resumable
content-addressed file sync (`rds send`/`rds recv`), an X11 desktop
pipeline (capture → H.264 → paced newest-wins stream → decode) behind
feature gates, a signed discovery directory, and transport metrics
with Prometheus export. Public API and wire protocol are not stable.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
