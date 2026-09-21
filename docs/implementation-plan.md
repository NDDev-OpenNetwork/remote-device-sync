# Implementation plan — owned connectivity core (v0.2) and beyond

The detailed engineering plan. `docs/roadmap.md` holds milestones and
gates; this file holds the workstreams: tasks, file-level scope, API
sketches, tests, exit criteria. Every fact about external crates/drafts
below was verified against vendored sources or upstream documents on
2026-09-21; where a fact is a projection, it is marked.

## 0. Doctrine

- **Harness first**: WS0 builds the measurement rig before any
  transport code, so every later change is measured, not argued.
- **No big-bang**: the iroh backend stays default until the noq backend
  beats or matches it on the same harness; selection is a config flag.
- **Both OSes always**: every task lands compiling and tested on Linux
  x86_64 and macOS arm64 (CI matrix enforces).
- **Docs-as-code**: a task is done when its doc sections and tests
  exist, not when the code compiles.
- Standards over invention: QUIC multipath is `draft-ietf-quic-
  multipath-21` (IESG-approved, RFC Editor queue); QNT inside noq is
  the n0 variant (`draft-bruynooghe-n0-quic-nat-traversal-00`) — we
  orchestrate it, we don't reimplement it.

## 1. Workstream map

| WS | Delivers | Crates touched | Depends on |
| --- | --- | --- | --- |
| WS0 | bench harness + baseline report | `rds-bench` (new) | — |
| WS1 | owned endpoint manager on noq | `rds-net` | WS0 (gating) |
| WS2 | owned relay protocol + server | `rds-relay`, `rds-server` | WS1 (client link) |
| WS3 | discovery service + publish/resolve | `rds-discovery`, `rds-server`, `rds-agent`, `rds-cli` | WS1 |
| WS4 | capability-based authz | `rds-core`, `rds-discovery`, `rds-agent` | WS3 |
| WS5 | session/media protocol v2 | `rds-core`, `rds-desktop` | WS1 |
| WS6 | sync transfer protocol | `rds-sync`, `rds-cli` | WS1 |
| WS7 | observability surface | `rds-net`, `rds-server`, `rds-agent` | WS1–WS3 |
| WS8 | deploy on gds-services + real E2E | estate side | all |

v0.3+ media/platform work (Vulkan Video, KMS, SCK, wgpu render) starts
only after WS1–WS3 gates pass.

## 2. WS0 — measurement harness

Without it "lowest latency" is unfalsifiable. New crate `rds-bench`:
library + `rds-bench` binary, dev-only (not shipped).

Tasks:

- **B1 — harness core**: spawn two endpoints in-process (per backend),
  optionally through an in-process relay; scenario runner producing a
  JSON report. Files: `crates/rds-bench/src/{lib,main,report}.rs`.
- **B2 — scenarios**: `connect` (cold: resolve→established),
  `ping` (RTT percentiles over N probes), `throughput` (bulk stream
  MB/s), `migrate` (kill socket path mid-transfer — simulated via mux
  drop, real netem variant later), `loss`/`jitter` via `tc netem`
  wrapper scripts (documented; run on Linux, skipped on macOS).
- **B3 — desktop-latency methodology**: frame carries
  `{capture_ts, encode_done_ts, send_ts}`; client stamps
  `{recv_ts, decoded_ts, presented_ts}`; input-to-pixel measured by a
  loopback mode where the agent flips a screen region on input and the
  client measures event→changed-frame latency. Shared-clock assumption
  is declared; cross-machine runs report RTT-decomposed estimates.
- **B4 — report format**: `docs/reports/bench-YYYYMMDD.md` template;
  p50/p95/p99, direct-vs-relay split, path counts.
- **Gate**: `rds-bench` produces a baseline report for the iroh
  backend in-process and over an in-process relay; committed under
  `docs/reports/baseline-iroh.md`.

## 3. WS1 — `rds-net::backends::noq` (owned transport)

Verified noq API surface this builds on (`noq` 1.3.0 vendored source):

- `Endpoint::new_with_abstract_socket(config, runtime, socket:
  Box<dyn AsyncUdpSocket>)` — the socket seam.
- `Connection::open_path(impl Into<FourTuple>, PathStatus)` →
  `OpenPath` future; `PathStatus::{Available, Backup}`; `path_events()`,
  `nat_traversal_updates()` streams; `observed_external_addr()`.
- `Connection::add_nat_traversal_address` / `remove_…` /
  `get_local/remote_nat_traversal_addresses`;
  `initiate_nat_traversal_round() -> Result<Vec<SocketAddr>, _>`
  (emits REACH_OUT to peer + probes candidates).
- `TransportConfig::max_concurrent_multipath_paths(u32)` enables
  multipath; `datagram_receive_buffer_size`/`datagram_send_buffer_size`
  enable QUIC datagrams; `congestion_controller_factory` (bbr3 in
  noq-proto); `AckFrequencyConfig`.

iroh's proven orchestration policy (from `remote_state.rs`), which our
endpoint manager replicates:

- `update_qnt_candidates`: diff local interface addrs ↔ QNT candidates;
  add/remove. Ours: feed `observed_external_addr` + interface addrs.
- `do_holepunching`: call `initiate_nat_traversal_round()`; retry on
  `NotEnoughAddresses`/`Multipath` errors after ~100 ms; fatal on
  `Closed`/`WrongConnectionSide`/`ExtensionNotNegotiated`.
- Discovery stream → `paths.insert_multiple(addrs)`; ours: resolve
  → `add_nat_traversal_address` + `open_path` per candidate.

### Tasks

- **N1 — socket mux** (`rds-net/src/backends/noq/socket.rs`):
  `MuxSocket` implementing `noq::AsyncUdpSocket`. Inner sends:
  real `UdpSocket` (v4+v6 where available) and a `RelayLink` channel
  (WS2). Received relay datagrams are surfaced with a synthetic
  remote addr from a private range (`fd00::/8` carve-out keyed by
  relay+peer) so QUIC sees them as ordinary paths. ECN off on relay
  path. `poll_recv` fans in from both sources with fairness.
- **N2 — endpoint manager** (`rds-net/src/backends/noq/endpoint.rs`):
  `Endpoint` wrapper: `bind(config)`, `connect(peer, addrs)`,
  `accept()`; reuses `rds-net` key persistence and ALPN `rds/0`.
  TLS: self-signed-cert-by-key scheme (same identity model as iroh:
  raw public key transported in the certificate). Config maps to
  `noq::TransportConfig`: multipath on, datagrams on (control+
  heartbeats), idle timeouts, BBR3 optional flag.
- **N3 — candidate pipeline** (`rds-net/src/backends/noq/path.rs`):
  on connect: add remote candidates from discovery record →
  `add_nat_traversal_address` + `open_path(_, Backup)` per direct
  addr (relay path is `Available` already). On
  `PathEvent::Established`: log + mark. On `ObservedAddr`: publish
  to discovery (WS3) and keep in candidate set.
- **N4 — traversal loop**: drive `initiate_nat_traversal_round` on
  connect and on `PathEvent::Closed`/network-change hint; the
  iroh-derived retry table above; expose attempts in metrics (WS7).
- **N5 — per-service path policy** (`rds-net/src/backends/noq/policy.rs`):
  map service kind → path preference (control/SSH: stable-first —
  prefer the path with lowest loss-variance; desktop video:
  lowest-RTT). Implemented by marking paths `Available`/`Backup`
  appropriately and, where scheduling control is needed, splitting
  services onto separate connections (documented fallback).
- **N6 — facade parity**: `rds-net` public API gains a backend
  selector (`EndpointConfig::backend = Iroh|Noq`); the iroh path is
  untouched. `rds-agent`/`rds-cli` get `--transport` flag.

### Tests

- unit: mux demux by source, candidate diffing, retry policy.
- integration: two noq endpoints in-process over real UDP loopback —
  connect, streams, datagrams; then via in-process relay link.
- parity: `rds-bench` suite on both backends side by side.

**Exit criteria (gate G1)**: noq backend passes the e2e suite that the
iroh backend passes (ping, tcp-forward, authz reject) AND bench parity:
connect time within 2× of iroh, RTT equal on direct, relay throughput
within 20% — or the delta is documented with a plan.

## 4. WS2 — owned relay protocol (`rds-relay::proto` → server)

Reference studied: iroh-relay 1.2 wire protocol (`protos/relay.rs`):
`ClientToRelayDatagram(+Batch)`, `RelayToClientDatagram(+Batch)`,
`EndpointGone`, `Ping/Pong`, `Status`, `Restarting`, `Health` — over an
HTTP-upgraded or QUIC connection. Ours is simpler: no HTTP legacy.

Wire design (ALPN `rds-relay/0`):

- One mutually-authenticated QUIC connection per endpoint↔relay.
- Control bidi stream: postcard-framed `RelayControl`:
  `Register { endpoint_key }`, `Registered { assigned: SocketAddr }`,
  `PeerGone { key }`, `Ping/Pong`, `Health { load, endpoints, drain }`,
  `Drain {}` (server → clients: migrate away).
- Payload: QUIC **datagrams** carrying `{dst_key}` + raw UDP payload —
  the relay is a packet mover; congestion control already applies to
  QUIC datagrams on the leg, which is the documented tradeoff (media
  avoids datagrams *between endpoints*; endpoint→relay datagrams are
  the relay's whole job — packets must move regardless).
- Server fan-out: `EndpointId → connection` map; `Forward` header parse
  is O(1); per-endpoint byte/packet counters.

Tasks:

- **R1** `rds-relay/src/proto.rs` — real message types + framing tests.
- **R2** `rds-relay/src/server.rs` — listener, register/forward tables,
  allowlist (reuse policy type), drain mode, health endpoint.
- **R3** `rds-net` `relay_link` — client side feeding `MuxSocket`.
- **R4** `rds-server` — serve relay proto + discovery (WS3) on one
  process; flags `--relay-addr`, `--dir`, `--allow`.
- Tests: register→forward roundtrip in-process; drop semantics;
  drain triggers client migration; forged dst_key dropped.

**Gate G2**: relayed throughput ≥ iroh-relay measured on WS0 harness
(target: ≥ its measured ~48 MiB/s class on loopback); drain removes the
relay from path selection without dropping the session.

## 5. WS3 — discovery service + registry bridge

- **D1** — `rds-server` HTTP API (or QUIC service stream; pick HTTP for
  ops simplicity): `PUT /v1/records` (verify → store; rate-limited),
  `GET /v1/records/{key}`, `DELETE`, `GET /v1/health`,
  `GET /v1/metrics`. Store: `rds_discovery::FileStore`.
- **D2** — agent publish loop (`rds-net` `announce` task): publish on
  start, refresh at `expires_at − TTL/3`, re-publish on
  `PathEvent::ObservedAddr` change.
- **D3** — resolve path: `rds <cmd> <target>` accepts
  `rds1…` ticket | bare key | GDS device name (registry bridge resolves
  name→key server-side). CLI `--server` points at the GDS host.
- **D4** — GDS registry bridge (private-estate side): the estate's
  device inventory maps `device_id → EndpointKey`; `rds-server` reads a
  signed registry snapshot to authorize name resolution. Public module
  ships only the record shape + store; the estate ships the bridge
  config. (Estate rules: private facts stay in the private repo.)
- Tests: publish→get roundtrip; expired record rejected by resolvers;
  forged record rejected by store (already unit-tested); TTL refresh
  keeps record live.

**Gate G3**: cold `rds ssh <name>` works end-to-end through discovery —
resolve→connect→first byte ≤ 300 ms on LAN, documented.

## 6. WS4 — capability authz

- Grant record in `rds-core`: `{issuer: Key, subject: EndpointKey,
  services: [Service], not_before, expires_at, constraints: {max_bps?,
  ports?, displays?}, signature}` — our own signed format (ed25519-dalek
  already in tree); biscuit deferred unless delegation chains prove
  needed.
- Enforcement: grant presented in the first control frame; agent
  verifies signature + expiry + service scope before opening service
  streams. Connection-level rejection = close before any stream
  service (same effect as `EndpointHooks::after_handshake`).
- Revocation: short TTL (minutes) + GDS denylist channel — the server
  pushes revoked grant hashes to agents on the control channel; agents
  also drop connections whose grants expired.
- Tests: expired grant rejected; wrong-service grant rejected;
  revoked grant rejected after denylist push.

## 7. WS5 — session/media protocol v2 (protocol only; codecs stay v0.3)

- Stream taxonomy (finalize in `rds-core`): `control` bidi (highest
  prio) — hello/authz/input/encoder-steer/heartbeat; `video` — one uni
  stream per frame (current design confirmed by research); `audio` —
  one uni stream, own clock; `sync` — bulk ordered streams.
- `FrameHeader` v2: `{seq, capture_ts, encode_done_ts, send_ts,
  keyframe, codec, w, h}` — powers the latency budget measurements.
- Input semantics: `InputEvent` gains `{event_ts, display_id}`;
  server-side acks optional (config flag for measurement mode).
- Pacing: encoder bitrate driven by `Connection::path_stats` loss/RTT +
  frame deadline misses (controller design in research.md §4).
- Tests: synthetic 240 fps frame generator through netem loss — queue
  depth never grows unbounded; stale-frame drop provable in logs.

## 8. WS6 — sync protocol

- `rds-sync` gains: `Session` (offer/request manifests),
  `missing_chunks` already done; `fetch` — chunk pull over dedicated
  streams (parallel N=4, resumable); `Journal` — resumable state file
  under the receive dir; `verify` — BLAKE3 per chunk + root.
- CLI: `rds send <peer> <path>`, `rds recv <peer> <dir>`, `rds sync <dir>`
  (two-way later; v1 is send/recv).
- Tests: 1 GiB random file in-process; kill mid-transfer, resume —
  byte-identical; identical content → zero chunks transferred;
  corrupt chunk → re-fetch.

## 9. WS7 — observability

- `rds-net` metrics: per-path RTT/loss/congestion, path events, QNT
  attempts/success, relay-vs-direct bytes. Facade over
  `iroh-metrics`-style counters; Prometheus export behind feature.
- `rds-server`: `/v1/metrics` scrape endpoint, per-endpoint accounting.
- Session event log: structured `tracing` spans per session with
  `session_id`, exported for bench reports.
- Gate: every number in `docs/reports/` is produced by the harness
  reading metrics — no hand-measured prose.

## 10. WS8 — deployment

- `rds-server` on `gds-services` (systemd unit, relay + directory).
- Estate side (private repo): device inventory gains `endpoint_key`;
  GDS policy ties allowlists to estate membership.
- Real-E2E: `rds ssh` between `nddev-amsterdam` and `gds-services`;
  desktop smoke on attended session; report committed.

## 11. Sequencing

```text
WS0 ──▶ WS1 ──▶ WS2 ──┬─▶ WS3 ──▶ WS4 ──▶ WS8
      (harness)  (endpoint) (relay) │        │
                                    └─▶ WS5  └─▶ WS7 (throughout)
                                        WS6 (parallel after WS1)
```

Parallelizable: WS6 after WS1; WS5 protocol bits after WS1; WS7 threads
through all.

## 12. Implementation-phase risks

- **Mux correctness**: relay datagrams must be presented to QUIC with a
  stable synthetic remote address; churn (peer changes relay) =
  abandon path + open new — documented behavior.
- **QNT edge cases**: symmetric NAT still fails to direct — the relay
  path must stay warm (`PathStatus::Backup` isn't enough; keep relay
  `Available` until direct proves stable for N seconds).
- **Relay trust**: relay learns topology (who talks to whom) — the
  documented privacy boundary is metadata, not content.
- **Scope creep**: WS5/WS6 are protocol-only; codec/capture hardware
  work is v0.3 — resisted inside v0.2.
- **macOS coverage**: noq socket/mux code is OS-portable; verify on
  macOS CI from the first commit, not at the end.

## 13. Decisions deferred to implementation

- HTTP vs QUIC for the discovery API (default HTTP — ops simplicity).
- One connection per service-class vs per-service path pinning (start
  with pinning; measure).
- Grant format: own postcard-signed vs biscuit (start own; swap if
  delegation is needed).
- Whether `rds-server` discovery API is on the same port as relay or
  separate (default: separate port, same process).
