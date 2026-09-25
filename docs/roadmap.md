# Roadmap

Current execution order and reopened gates are in the
[2026-09-24 remediation plan](remediation-plan.md), backed by the
[implementation/session audit](reports/rds-audit-20260924.md).
The milestones below preserve the original direction; historical checkpoint
completion does not establish that the audited defects or product gaps are closed.

The [observability sequence](observability.md) now accompanies remediation:
shared Rust telemetry, Vector/OpenObserve qualification, then secured source
metrics, diagnostic bundles and private operational rollout. Its first local
pipeline does not close product or release gates.

Each workstream ends at a named **checkpoint** — a reproducible
verification bundle (functional, simulation, impairment, interop,
fuzz, soak, security layers as applicable) whose report is committed
to `docs/reports/` before the next wave starts. Order is chosen so
the riskiest owned layers (transport, relay) are proven early. The
task-level plan, checkpoint protocol and per-gate check tables live
in [implementation-plan.md](implementation-plan.md).

## v0.1 — foundation (done)

Working SSH/TCP forwarding over direct and relayed QUIC (iroh backend),
endpoint allowlists, X11/OpenH264 desktop skeleton, workspace structure.

## v0.2 — owned connectivity core

- `rds-net::backends::noq`: socket mux over `AsyncUdpSocket`, endpoint
  manager, path-open policy, `PathEvents`/`NatTraversalUpdates` wiring.
- `rds-relay::proto`: our relay protocol (REGISTER/FORWARD, datagram
  channel); `rds-server` serves it beside the iroh relay.
- `rds-discovery`: `rds-server` exposes PUT/GET of signed
  `EndpointRecord`s; agent publishes; `rds ssh <device>` resolves
  through GDS instead of pasted tickets.
- Authz: connection-level check (equivalent of `EndpointHooks::
  after_handshake` in our layer).
- **Gates**: same-harness parity vs iroh backend — setup time,
  time-to-direct, RTT, relayed throughput, migration survival; feature
  flag picks backend per run.

## v0.3 — media pipeline, phase 1

- Damage-driven capture (VFR: encode only what changed) — the single
  biggest latency/bandwidth win.
- Linux: `ext-image-copy-capture` backend, then DRM/KMS unattended path.
- macOS: ScreenCaptureKit + VideoToolbox backend (new platform bring-up).
- wgpu renderer (newest-frame-only), Vulkan Video H.264 encode/decode
  behind probe; OpenH264 stays the floor.
- Input: portal EIS (`reis`) on Wayland; `CGEvent` on macOS.
- **Gates**: glass-to-glass ≤20 ms LAN / ≤80 ms WAN at 1080p60
  measured end-to-end; input-to-visible-pixel budget tracked.

## v0.4 — sync and resilience

- `rds-sync` transfer protocol on our streams: manifest exchange,
  `missing_chunks` delta, resume journal; `rds send/recv`.
- Multi-relay map + failover, drain signalling, relay metrics surface.
- Adaptive bitrate from per-path congestion state; FEC behind measured
  loss; audio (`rds-audio` Opus path).
- **Gates**: loss/jitter harness (`netem`) — 5% loss, 30 ms jitter:
  desktop stays interactive; sync resumes mid-transfer; relay failover
  does not drop sessions.

## v0.5+ — interop and breadth

- RDP frontend (`ironrdp-server`) for stock RDP clients.
- Windows support (DXGI + MF/D3D11 + SendInput) behind existing seams.
- Browser client (WebRTC/str0m gap-fill) only if a real need appears.
- HEVC/AV1 negotiated tiers; multi-viewer broadcast (moq-lite) only if
  the product needs it.

## Standing quality bars

- CI green on `ubuntu-latest` + `macos-latest`; fmt/clippy/test +
  `cargo deny` licenses.
- Every milestone ends with a `docs/reports/` entry: what was measured,
  on what network, p50/p99 numbers.
- `docs/research.md` §7 risk register revisited at each gate.
