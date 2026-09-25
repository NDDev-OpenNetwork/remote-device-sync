# remote-device-sync

Remote device access for the GDS estate: connect from any enrolled device to
another over SSH, with remote desktop sessions — transport is QUIC via
[iroh](https://crates.io/crates/iroh), so peers are Ed25519 keys, direct
paths are hole-punched, and a self-hosted relay covers egress-only
networks. See [docs/architecture.md](docs/architecture.md) for the research
and protocol decisions.

Current readiness: the [2026-09-24 audit](docs/reports/rds-audit-20260924.md)
identifies authorization, discovery, sync and owned-transport blockers.
The [remediation plan](docs/remediation-plan.md) defines their fixes and the
remaining native SSH, interactive desktop and platform work.

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
├── rds-client     shared client operations and local session manager/IPC
├── rds-observe    bounded process logs and privacy-preserving telemetry
├── rds-bench      development measurement/qualification harness
├── rds-desktop    capture/codec/input/render traits + platform backends
├── rds-audio      Opus audio pipeline (scaffold)
└── rds-sync       FastCDC+BLAKE3 content-addressed sync
```

Docs: [architecture](docs/architecture.md) · [deep research](docs/research.md)
· [platforms](docs/platforms.md) · [conventions](docs/conventions.md) ·
[roadmap](docs/roadmap.md).

## Usage

For multiple persistent device connections, enable the agent's opt-in
[local session manager](docs/local-sessions.md). `rds session` lists/selects
devices and opens SSH/TCP forwards through the running agent's shared endpoint.
The direct compatibility commands below still create independent endpoints.

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
rds desktop <ticket>           # headless decode/stats; --features desktop on both ends

# Self-hosted relay instead of the public n0 relays:
rds-relay --addr 0.0.0.0:3340
rds-agent --relay http://relay.host:3340 ...
rds --relay http://relay.host:3340 ...

# rds-server composes relay + signed discovery directory on one host
# (see docs/deployment.md for the systemd units and firewall rules):
rds-server --relay-addr 0.0.0.0:3340 --http-addr 0.0.0.0:3341 \
    --directory /var/lib/rds/directory --allow <endpoint-id> \
    --directory-allow <base32-device-key>

# Name lookup requires a registry key provisioned through GDS/configuration:
rds --server https://directory.example.com:3341 --registry-key <base32-verifying-key> ping device-a
```

Both relay binaries also support `--relay-backend noq` when built with
`owned-relay`. This mode requires a separate persistent `--relay-key-file` and
an explicit peer allowlist; an open test relay requires
`--development-open-relay`. Client and agent use `--backend noq --owned-relay`
with the relay's pinned public identity and address. See the
[relay runtime contract and build examples](docs/relay-runtime.md).
The iroh backend remains the default while owned connectivity qualification
is in progress.

The directory requires configured publishers (`--directory-allow`,
repeat per device). An empty list denies record publish/fetch/delete. A signed
reachability record or name binding does not enroll a device.

Remote-session traffic is end-to-end encrypted between the endpoints; the
relay sees ciphertext. Directory HTTPS runs in-process with certificate and
hostname verification; use `--directory-ca` for a private CA. Signed name proofs
remain required independently of TLS. Explicit HTTP/IP:port remains available
for local deployments; HTTPS never falls back to it. The agent rejects any peer not on its `--allow` list before
a service stream opens, and can additionally require estate-signed capability
grants (`--issuer`, scope-checked per stream). Managed grant mode requires
`--directory` and `--revocations-key`; stale policy closes access. Directory,
agent and name CLI persist their policy revisions and freshness state. See
[policy configuration and migration](docs/policy-state.md) before upgrading
the issuer or deploying managed access.

Endpoint publication also persists its revision counter and exact retry bytes
(`rds-agent --record-state`, default beside the identity key). Records and
deletes use a new versioned signature contract; old directories require an
explicit migration that is still being implemented. See
[record state and migration limits](docs/record-state.md) before deployment.

Observability: all binaries share bounded stderr logging. Set
`RDS_LOG_FORMAT=json` for the allowlisted export consumed by the supplied
Vector/OpenObserve pipeline; local `text` diagnostics can contain private data.
Process/session correlation, observed operation latency, loss counters and
alert definitions have a disposable integration fixture. See the
[contract, checks and remaining rollout plan](docs/observability.md).
`rds-net` separately exposes observed path counters and coverage flags, with
Prometheus rendering behind `metrics`. Daemons can expose aggregate source
metrics through a separate authenticated loopback listener, using paired
`--admin-addr`/`--admin-token-file` flags. Create a credential with
`rds admin-token --file PATH`. The public `/v1/metrics` route is removed;
source coverage and unknown values are explicit.

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

v0.1 foundation: SSH/TCP forwarding, a single-file transfer engine, a
feature-gated X11 capture/H.264/headless-decode pipeline, discovery and grant
primitives, and transport metrics. The desktop viewer and native macOS/Wayland
backends remain incomplete. Known correctness and security blockers are
tracked in the audit above; this is not a production-readiness claim.
Public API and wire protocol are not stable.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
