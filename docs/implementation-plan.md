# Implementation plan — owned connectivity core (v0.2) and beyond

Follow-up execution: [remediation plan](remediation-plan.md), based on the
[2026-09-24 audit](reports/rds-audit-20260924.md). It identifies reopened
acceptance criteria and remaining product work. This original WS0–WS8 plan
and its dated reports are historical requirements/evidence, not a claim that
all current behavior satisfies them.

The owner's 2026-09-25 observability addition is tracked in the
[observability contract and O1–O6 execution sequence](observability.md), under
remediation W10.1/W10.2. It uses shared Rust process telemetry with replaceable
Vector/OpenObserve infrastructure and does not close historical C7 by itself.
The durable catalog/policy observation increment is recorded in its
[2026-09-25 receipt](reports/rds-durable-metrics-20260925.md); source coverage
remains partial and the O3–O6 gates still apply.

The detailed engineering plan. `docs/roadmap.md` holds milestones and
gates; this file holds the workstreams: tasks, file-level scope, API
sketches, tests, exit criteria. Every fact about external crates/drafts
below was verified against vendored sources or upstream documents on
2026-09-21; where a fact is a projection, it is marked.

## 0. Doctrine

The current product-facing increment is the W2.4
[local session manager](local-sessions.md). Default CLI connectivity and local
key-inode runtime ownership are implemented. Remaining manager work covers
viewer/sync APIs and installed-device qualification. The [native SSH client](ssh.md)
now covers standard shell/exec/PTY requests; host-key/account enrollment,
broker/reattachment and real-network/platform qualification remain W5 work,
alongside viewer/input switching. This supplements the complete remediation plan;
the manager does not close a wave or replace WS0 acceptance requirements.

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
- **Every wave ends at a checkpoint** (§Checkpoint protocol): a named, reproducible
  verification bundle whose evidence is committed to
  `docs/reports/`. A wave is not "mostly done" — its gate is either
  passed with artifacts or the wave stays open.

## Checkpoint protocol

A **checkpoint** is the contract that closes a wave. It exists to make
"it works on my machine" impossible: every gate emits artifacts, the
artifacts are committed, and the automatable parts join the regression
suite so later waves cannot silently break earlier guarantees.

### The gate procedure

```text
entry:   prior checkpoints' regression suite green on main
build:   the wave's tasks complete (code + docs + tests)
run:     scripts/checkpoint.sh <gate>  → docs/reports/checkpoint-<id>.md
review:  the report's checklist is filled honestly (an unrun check is
         marked "not run", never "assumed")
merge:   PR to main; the checkpoint report is part of the diff
```

`scripts/checkpoint.sh` runs the automatable parts and writes the
report skeleton. Failing any check = fix forward or revert — main never
carries a known-broken gate.

### The check layers

Every checkpoint draws from this fixed menu; the per-wave tables below
say which apply.

| Layer | What it proves | Tooling |
| --- | --- | --- |
| **green bars** | compile/lint/test on both OSes | CI matrix (`fmt`, `clippy -D warnings`, `test`) — always required |
| **functional** | the wave's behavior | new unit/integration tests + all prior ones |
| **simulation** | correctness under partition/loss, deterministically | `turmoil` hosts in-process (seeded RNG, hold/release/partition); committed seeds |
| **impairment** | behavior on real bad networks | `tc netem` matrix on loopback: loss {0,1,5}%, jitter {0,30}ms, bw cap {10,100}M; `toxiproxy` for the TCP control plane; `quic-network-simulator` (ns3+docker) as the heavy option |
| **interop** | old↔new compatibility | cross-backend matrix (iroh↔noq endpoints once WS1 lands), prior↔current ALPN handshake |
| **fuzz** | parsers don't panic/misbehave on garbage | `proptest` round-trips + `bolero`/`cargo-fuzz` corpus runs on every wire decoder touched |
| **soak** | no leaks/degradation over time | 30-min session: steady memory, stable latency percentiles, zero reconnects unless injected |
| **security** | authz holds, DoS surface bounded | checklist: signature coverage, expiry paths, rate limits, bounded queues, `unsafe` audit |

### The bench case taxonomy (adopted from quic-interop-runner)

Our impairment/interop cases reuse the QUIC interop runner's proven
set, mapped to rds semantics: `handshake`, `transfer` (flow control +
multiplexing), `multiconnect` (handshake under high loss),
`rebind-port`, `rebind-addr` (NAT rebinding → path validation),
`migration` (active path switch mid-session), plus our own
`relay-fallback`, `relay-drain`, `discovery-refresh`.

### Regression lock

Each checkpoint's automatable checks are added to `tests/` and the
`rds-bench` scenario set, marked with the gate id. CI runs the fast
subset per PR; the full matrix runs on a nightly/scheduled workflow.
A gate id is a promise: wave N+1 must keep wave N's checks green.

## 1. Workstream map

| WS | Delivers | Crates touched | Checkpoint | Depends on |
| --- | --- | --- | --- | --- |
| WS0 | bench harness + checkpoint tooling | `rds-bench` (new), `scripts/checkpoint.sh` | C0: baseline trustworthy | — |
| WS1 | owned endpoint manager on noq | `rds-net` | C1: parity + survival | C0 |
| WS2 | owned relay protocol + server | `rds-relay`, `rds-server` | C2: relay parity + drain | C1 |
| WS3 | discovery service + publish/resolve | `rds-discovery`, `rds-server`, `rds-agent`, `rds-cli` | C3: resolve + hostile input | C1 |
| WS4 | capability-based authz | `rds-core`, `rds-discovery`, `rds-agent` | C4: boundary airtight | C3 |
| WS5 | session/media protocol v2 | `rds-core`, `rds-desktop` | C5: latency under loss | C1 |
| WS6 | sync transfer protocol | `rds-sync`, `rds-cli` | C6: never corrupts | C1 |
| WS7 | observability surface | `rds-net`, `rds-server`, `rds-agent` | C7: numbers are real | C1–C3 |
| WS8 | deploy on directory-host + real E2E | estate side | C8: real metal | all |

v0.3+ media/platform work (Vulkan Video, KMS, SCK, wgpu render) starts
only after C1–C3 pass.

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
- **B5 — checkpoint tooling**: `scripts/checkpoint.sh <gate>` — runs
  the gate's automatable layers, writes the report skeleton to
  `docs/reports/checkpoint-<id>.md`, exits non-zero on any failed
  check. This wave owns the machinery every later checkpoint uses.

**Checkpoint C0** — the measurer is trustworthy:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass on both OSes |
| functional | all scenarios run on loopback; JSON report parses; p50/p95/p99 populated |
| impairment | netem matrix script proven end-to-end: 9-cell run (loss {0,1,5}% × jitter {0,30}ms × bw {10,100}M) yields distinct, plausible numbers — sanity-checked (more loss ⇒ not less RTT) |
| docs | `docs/reports/checkpoint-c0.md` + `baseline-iroh.md` committed; methodology section explains shared-clock limits |

Exit = the baseline report exists and a second run reproduces it
within measurement noise (declared threshold, e.g. p95 within 15%).

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

**Checkpoint C1** — the owned transport earns its place:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass; `--features transport-noq` builds on both OSes |
| functional | noq backend passes the same e2e suite as iroh (ping, tcp-forward, authz reject) — the suite is backend-parameterized |
| simulation | turmoil: seeded partition/heal between endpoints — connection survives partition, streams resume on heal; committed seeds in the test |
| impairment | netem matrix on both backends: `handshake`, `multiconnect` (handshake under 5% loss), `rebind-addr`, `migration`, `relay-fallback` — noq no worse than iroh per case |
| interop | iroh endpoint ↔ noq endpoint on `rds/0`: connect + ping both directions — proves wire-level standards compliance, not self-consistency |
| fuzz | `proptest`/`bolero` on candidate-record parse + mux demux of malformed datagrams (garbage relay payloads can't panic the socket) |
| soak | 30-min noq session over relay with a direct-path flap every 60 s: steady RSS, p99 RTT drift < 20%, zero deadlocks |
| security | checklist: amplification guard (relay path usable only after handshake), candidate count cap, `open_path` rate limit, unsafe audit |

**Gate G1**: C0+C1 checks green AND bench parity — connect ≤2× iroh,
RTT equal on direct, relay throughput within 20% — or the delta is
documented with a plan and the default stays iroh.

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

**Checkpoint C2** — our relay replaces iroh-relay without regression:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass |
| functional | register→forward roundtrip, drop semantics, `PeerGone` on disconnect, forged `dst_key` dropped |
| simulation | turmoil: relay host killed mid-session → clients get `PeerGone`/conn close, reconnect to second relay, streams resume |
| impairment | relay leg under netem 5% loss — throughput collapse is bounded and logged (datagram leg has no retransmission; expected degradation documented) |
| interop | rds-relay client ↔ iroh endpoint over relayed path; rds client ↔ iroh-relay (cross-compat where wire formats overlap — otherwise documented N/A) |
| fuzz | relay control-stream decoder: truncated/oversized/forged frames never panic; datagram parser rejects >MTU and malformed headers |
| soak | 30-min, 100 endpoint churn (register/deregister loop) — table sizes bounded, RSS steady |
| security | rate limit on register + datagram flood per endpoint; drain actually stops new registrations; unauthenticated payload refused |

**Gate G2**: relayed throughput ≥ iroh-relay on the WS0 harness
(~48 MiB/s class on loopback); drain removes the relay from path
selection without dropping live sessions.

## 5. WS3 — discovery service + registry bridge

- **D1** — `rds-server` HTTP API (or QUIC service stream; pick HTTP for
  ops simplicity): `PUT /v1/records` (verify → store; rate-limited),
  `GET /v1/records/{key}`, `DELETE`, `GET /v1/health`. Store:
  `rds_discovery::FileStore`. The original `/v1/metrics` route was removed by
  remediation O2; metrics use the separate authenticated admin listener.
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

**Checkpoint C3** — discovery is correct and hostile-input safe:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass |
| functional | publish→get roundtrip; expired record refused by resolvers; TTL refresh keeps record live; `ObservedAddr` re-publish works |
| impairment | toxiproxy on the discovery HTTP API: latency toxic → resolver timeout behaves; down toxic → clean cached-ticket fallback error |
| fuzz | record parser + HTTP handlers: garbage bodies, oversized payloads, replayed old records — all rejected, none panic |
| security | signature coverage 100% on stored records (store refuses unsigned/forged); PUT rate limit verified; replay protection (older `issued_at` rejected) |

**Gate G3**: cold `rds ssh <name>` works end-to-end through discovery —
resolve→connect→first byte ≤ 300 ms on LAN, measured by the harness.

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

**Checkpoint C4** — authz is airtight at the boundary:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass |
| functional | expired / wrong-service / revoked grants rejected; valid grant for service A cannot open service B's stream |
| fuzz | grant decoder under malformed input (truncated signature, huge service list) — reject, never panic |
| security | negative-test coverage: every authz path has a forge test; grant-replay across connections rejected; clock-skew tolerance documented |

**Gate G4**: no service stream opens before grant verification —
asserted by a test that opens streams concurrently with the control
frame and counts rejections.

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

**Checkpoint C5** — the media protocol carries real pressure:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass |
| functional | header roundtrip; stream-priority order observed (control beats video under congestion); keyframe-request roundtrip |
| impairment | netem 5% loss + 30 ms jitter: frame queue bounded, newest-frame-wins presentation, latency percentiles in report |
| fuzz | `FrameHeader` decoder + stream demux on malformed data |
| soak | 30-min synthetic stream at 60 fps: steady RSS, frame-age p99 bounded, zero unbounded-queue events |

**Gate G5**: under 5% loss the viewer-visible latency stays within the
declared budget (initial: ≤150 ms p95 in-process) and control stream
latency is unaffected by video backlog.

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

**Checkpoint C6** — sync never corrupts, always resumes:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass |
| functional | 1 GiB transfer; kill at random points → resume → byte-identical; identical → zero chunks; corrupt chunk → re-fetch |
| simulation | turmoil-fs or fault hooks: torn journal write, partial chunk file — resume still correct |
| impairment | netem 5% loss + disconnect every 30 s — transfer completes, resume overhead < 5% re-fetched |
| fuzz | manifest + chunk decoders on malformed input |
| security | BLAKE3 verify coverage: every stored chunk verified before use; path-traversal in manifest entries rejected |

**Gate G6**: transfer survives worst scripted failure (kill -9 at a
random offset) with byte-identical result — run 20×, all pass.

## 9. WS7 — observability

- `rds-net` metrics: per-path RTT/loss/congestion, path events, QNT
  attempts/success, relay-vs-direct bytes. Facade over
  `iroh-metrics`-style counters; Prometheus export behind feature.
- `rds-server`: separate authenticated loopback `GET /metrics`, aggregate
  accounting. Remediation O2 supersedes the original public-listener route and
  removes stable per-writer labels; see [current contract](observability.md).
- Session event log: structured `tracing` spans per session with
  `session_id`, exported for bench reports.

**Checkpoint C7** — observability is load-bearing, not decorative:

| Layer | Check |
| --- | --- |
| green bars | CI matrix pass |
| functional | every metric the bench report cites exists in the export; counter accuracy proven by a known-traffic test |
| security | admin metrics require a separate loopback listener and bearer authentication; the public listener returns 404 even through a local proxy; no keys/secrets/peer labels/content |

**Gate G7**: every number in `docs/reports/` is produced by the harness
reading metrics — no hand-measured prose.

## 10. WS8 — deployment

- `rds-server` on `directory-host` (systemd unit, relay + directory).
- Estate side (private repo): device inventory gains `endpoint_key`;
  GDS policy ties allowlists to estate membership.
- Real-E2E: `rds ssh` between `device-a` and `directory-host`;
  desktop smoke on attended session; report committed.

**Checkpoint C8** — the system works on real metal, not just in sims:

| Layer | Check |
| --- | --- |
| functional | `rds ssh` across real NAT (device-a ↔ directory-host): connect, run commands, survive a relay↔direct transition |
| impairment | real-network report: measured RTT/loss/path used, compared against harness predictions — deltas explained |
| soak | 1-hour real session: reconnects counted, RSS steady on both ends |
| security | deploy review: systemd sandboxing (ProtectSystem, NoNewPrivileges), key permissions 0600, ports/firewall documented |
| docs | runbook: restart/upgrade/drain procedures; failure modes table |

**Gate G8**: real-device e2e report committed; every previous gate's
automated checks still green on the deployed binaries.

## 11. Sequencing

```text
C0 ──▶ WS1 ──▶ C1 ──▶ WS2 ──▶ C2 ──┬─▶ WS3 ──▶ C3 ──▶ WS4 ──▶ C4 ──▶ WS8 ──▶ C8
(harness)       (endpoint)  (relay)│                                     │
                                   └─▶ WS5 ──▶ C5    WS7 ◀── threads ────┘
                                       WS6 ──▶ C6 (parallel after C1)
```

Rules:

- A wave starts only after its dependency's checkpoint report is
  merged — `docs/reports/checkpoint-*.md` on main is the unlock token.
- Checkpoint automation lands with the wave that introduces the check;
  `scripts/checkpoint.sh` refuses a gate id with no registered checks.
- Parallelizable: WS6 after C1; WS5 after C1; WS7 threads through all.
- If a checkpoint fails after merge (flaky found later), the fix
  carries the failed artifact + the fix evidence — the report is
  amended, not deleted.

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
