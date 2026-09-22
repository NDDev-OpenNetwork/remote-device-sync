# Changelog

## [Unreleased]

- Protocol v3 + review hardening: every uni-directional stream now
  opens with a `UniHello` tag (`Desktop`/`Sync`/`Audio`), and the
  accepting side routes it through a single per-connection demux
  (`Connection::uni_streams`) — desktop and sync can share a
  connection without racing `accept_uni`. Inboxes end cleanly:
  `UniStreams::recv` returns `None` once the connection dies — the
  demux's exit drops every registered sender instead of leaving
  consumers parked. `PROTOCOL_VERSION` is 3; v2 peers won't interop
  on uni streams.
- Security fixes: sync confinement is now resolved, not just lexical —
  `resolve_under` canonicalizes the deepest existing ancestor of every
  destination and requires it to stay under the canonical sync root,
  so symlinked components can't redirect pulls, the `.rds-sync`
  journal, or assembly outside the root; assembly's temp file is a
  suffixed `*.rds-part` created exclusively after unlinking any stale
  one, and `.rds-sync` as a first path component is refused outright.
  `ChunkHdr.len` is checked against the manifest before it sizes the
  receive buffer (a forged u32 no longer forces a huge allocation).
  X11 scroll injection clamps deltas to 32 clicks per event. Input
  events naming a display other than the session's are dropped
  un-acked. The directory's `/v1/revocations` PUT now verifies and
  stores under one write lock (no check-then-store regression window).
- Correctness fix: `mailbox::Sender` decrements an explicit sender
  count and then notifies on drop — a consumer parked in `recv` now
  observes the last sender leaving instead of sleeping forever, and a
  cross-thread wake can't observe a stale count and re-park
  (regression tests `parked_recv_wakes_when_last_sender_drops`,
  `parked_recv_survives_drop_wake_race`).
- CI/CD: the hand-rolled `ci.yml` is replaced by pinned
  `ci-workflows` reusables (0.1.26): `rust-ci` (locked build, fmt,
  five clippy lanes, ubuntu+macos test matrix), `rust-supply-chain`
  (cargo-deny per `deny.toml`, cargo-audit, cargo-machete — weekly
  advisory sweep), `public-codeql` (rust + actions, build-mode none)
  and `release-supply-chain` (tag `X.Y.Z` → immutable release: source
  archive + SPDX SBOM + SHA256SUMS + SLSA/SBOM attestations, behind a
  `release` environment). `deny.toml` now allows `Unlicense` and
  `CDLA-Permissive-2.0` (iroh transitive deps) and ignores the two
  unfixable unmaintained advisories; 13 unused crate dependencies
  removed; `VERSION` file added for the release contract; dependabot
  tracks cargo + github-actions weekly.
- WS8 deployment: `deploy/systemd/rds-server.service` and
  `rds-agent.service` — hardened units (ProtectSystem=strict,
  NoNewPrivileges, PrivateTmp/Devices, ProtectKernel*/ControlGroups,
  empty CapabilityBoundingSet, @system-service filter,
  AF_INET/6/UNIX/NETLINK only, UMask=0077, StateDirectory-scoped
  writes). `docs/deployment.md` documents install, ports/firewall
  (3340/tcp relay, 3341/tcp directory, metrics loopback-only),
  restart/upgrade/drain procedures and a failure-modes table.
  `rds ping` now prints per-path stats (via/selected/RTT/sent/lost/
  cwnd) for ops evidence; `rds-net/examples/ticket` mints
  restricted-address tickets (e.g. relay-only) for path verification.
  Deployment findings encoded: AF_NETLINK is required for interface
  monitoring; a client transiting the relay must itself be on the
  relay's `--allow`; an agent and a CLI on one host need distinct key
  files or the relay disconnects the duplicate EndpointId.
- WS7 observability: `rds-net::metrics` — a per-endpoint `Registry`
  of atomic counters plus a per-connection `ConnSampler` that folds
  cumulative `path_stats()` deltas into them. The
  `via="direct"`/`via="relay"` split is exact across path migration:
  datagrams and bytes sent/lost, congestion events, paths seen,
  connection totals, and RTT/cwnd/live-paths/active-connections
  gauges. QNT attempt/success counters are driven by the noq policy
  driver (iroh does not expose hole-punch attempts; a `direct` path
  appearing after a relay-only start is the equivalent signal).
  `Registry::render_prometheus` emits text exposition behind the new
  `metrics` feature. `PathStats` gains `sent_bytes`/`recv_bytes`.
  `rds-server`'s `GET /v1/metrics` now reports per-endpoint PUT
  counters under anonymized 16-hex BLAKE3-prefix labels and answers
  loopback peers only — raw keys, peer addresses and content are
  never exposed; remote scraping goes over SSH or a local exporter.
  The agent wraps every connection in an `rds.conn{peer, session_id}`
  span with nested `rds.stream{service}` spans and runs a 1s
  `ConnSampler` per connection; `rds-sync` logs session-boundary
  events with byte/chunk counts. `rds-bench` reports embed the
  `client_`/`agent_` registry snapshot collected at run time, so
  every reported number is backed by the harness's own counters.
- WS6 content-addressed sync: `rds-sync` gains a real protocol —
  FastCDC chunking, BLAKE3 per-chunk + per-file roots, a bounded
  wire format (`Offer`/`Request`/`Refuse`, `ManifestPart` ≤512
  entries, `Need` bitmap ≤256K chunks, `ChunkSet` ≤4096 indices,
  chunk payloads on 4 dedicated uni streams, `SetDone`/`Done`).
  Receives journal under `<dest>/.rds-sync/<root>/`: surviving parts
  are re-verified by content hash (corrupt parts refetched), a torn
  meta is rebuilt from the offer, an existing destination file seeds
  `have` so identical resends move zero bytes, and assembly is an
  atomic rename after root verification. `rel_path` rejects
  traversal, absolute paths, NUL and oversize. `rds send`/`rds recv`
  push/pull through `rds_cli::open_sync`; the agent serves `Sync`
  under `--sync-dir` with a one-session-per-connection guard and
  advertises `Sync` in
  `Info` only when configured. E2E: byte-identical push/pull,
  zero-chunk resend, corrupt-part refetch, torn-journal and
  mid-transfer kill resume, repeated kill/resume convergence,
  traversal fuzz, and a lossy-socket impaired-lane completion.
- WS5 session/media protocol v2: `FrameHeader` gains
  `capture_ts_ms`/`send_ts_ms` (shared-clock latency measurement),
  `DesktopControl` gains heartbeat + `RequestIdr` + `SetBitrate`,
  `DesktopEvent` gains input acks + heartbeat echoes, and
  `InputEvent` carries `seq`/`ts_ms` metadata. The desktop session
  now runs a producer/writer pipeline with bounded collapse
  (keyframe-preserving), serialized sends token-bucket-paced to the
  adaptive `BitrateController` (RTT/loss/deadline-miss driven), and
  a receiver that drops frames below a next-expected-seq watermark,
  auto-requests IDR on delivered-seq gaps, and re-arms IDR when
  backpressure kills a keyframe. Per-frame stream priorities were
  removed after they starved in-flight streams under load; the
  control stream keeps max priority. `rds-net` gains a normalized
  `PathStats` facade on both backends (RTT, cwnd, sent/lost, relay
  flag) plus `EndpointConfig::without_discovery()` /
  `with_path_pinning()` and the feature-gated
  `bind_noq_with_socket` seam. `rds-bench` gains `ImpairingSocket` —
  an `AsyncUdpSocket` decorator applying seeded loss/delay/jitter
  *beneath* QUIC so path migration cannot bypass it. Tests: 5-test
  `session_v2` e2e suite on both transports proving keyframe
  roundtrip, 240fps newest-wins collapse, input-ack/heartbeat RTT,
  socket-verified impairment with split queue/wire latency gates,
  and a 60fps soak (`RDS_SOAK_SECS` for the 30min checkpoint run).
- Post-WS3 consistency + hardening: `rds-desktop` now programs
  against the `rds_net` facade (`Connection` plus the stream/error
  re-exports — the documented "+rds-net when streams abstract"
  direction), so the `desktop` feature on `rds-agent`/`rds-cli`
  compiles and serves on both transport backends instead of failing
  on a raw-`iroh` type mismatch; a new CI lane
  `--features rds-agent/desktop,rds-cli/desktop` (both OSes) keeps it
  compiling. Session teardown fixed: `serve_desktop` aborts the
  frame-writer task and the capture producer exits when its channel
  closes — `JoinHandle::abort` cannot interrupt `spawn_blocking`
  work, so ending a session previously leaked a permanently-spinning
  capture/encode thread; `DesktopSession` drop aborts the
  frame-receiver task so the connection closes with the session
  instead of outliving it. The directory's global per-minute window
  now covers every signature-verifying write — `PUT /v1/records`,
  `PUT /v1/registry`, `DELETE /v1/records/{key}` — checked before
  parsing, exported as `rds_directory_writes_rate_limited`.
  `x11rb` 0.13 → 0.14.
- WS3 discovery service + publish/resolve (`rds-discovery`):
  `EndpointRecord` payloads carry `issued_at`; stores reject
  forged, expired and same-age-or-older replayed records, and honor
  signed `DeleteRequest` tombstones with replay protection. Minimal
  owned HTTP/1.1 codec (`http.rs` — hard head/body caps, one request
  per connection) serving `PUT/GET/DELETE /v1/records`,
  `GET /v1/names/{name}`, `PUT /v1/registry`, `/v1/health`,
  `/v1/metrics` with per-key + global PUT rate limiting
  (`service.rs`), plus a matching timeout-bounded `client.rs`.
  Estate name→key resolution rides a signed `SignedRegistry`
  snapshot (`registry.rs`) verified against a configured registry
  key — the directory can withhold but never invent names.
  `rds-net`: `announce` task publishes on start, refreshes at
  `ttl/3`, and republishes promptly when the advertised address set
  changes; `resolve_target` resolves ticket → bare key → device
  name. `rds` CLI gains `--server`; `rds-agent` gains `--directory` /
  `--record-ttl`; `rds-server` composes relay + directory + registry
  flags. `rds-bench` gains the G3 `resolve-connect` scenario: cold
  name→resolve→connect→first-byte per fresh endpoint. Gate `c3`:
  G3 p50 well under the 300 ms budget (see
  `docs/reports/checkpoint-c3.md`).
- WS2 owned relay transport (`rds-relay` + `rds-net::backends::noq::relay`):
  shared wire protocol in `rds_core::relay` (`rds-relay/0` ALPN, control
  stream, `[32B key][payload]` datagram frames); owned server with
  per-endpoint slots keyed by TLS-verified identity, token-bucket rate
  limiting, `PeerGone` fan-out, drain broadcast, identity-checked
  replacement. Client side: `RelaySocket` joins the socket mux as a
  tunnel child — a same-key helper endpoint registers on the control
  bidi, peers map to synthetic `198.19.0.0/16` addresses, and
  `EndpointConfig::relay_endpoint` attaches it. e2e proves handshake +
  datagrams + streams entirely through the tunnel, stale-slot
  replacement, and drain eviction. Gate `c2` passed; multi-relay
  failover deferred to WS3.
- WS1c connection driver + path migration: per-connection
  `connection_driver` task (`policy.rs`) consumes QNT address
  advertisements (`ADD_ADDRESS`/`REMOVE_ADDRESS`) and path events, and
  applies a biased-RTT path selection — the lowest-RTT path becomes
  `Available`, all others `Backup`, with a 5ms stickiness margin to
  prevent flapping. Holds only `WeakConnectionHandle`/`WeakPathHandle`
  so the task never keeps a connection alive (last-handle-drop stays the
  close trigger). Both endpoints advertise their real socket addrs and
  the client initiates traversal rounds, so peers can upgrade paths
  learned in-band. Result: under the `impaired` scenario (5% loss,
  50+30ms delay via proxy) noq now migrates traffic to a direct path —
  p50 1.80ms vs 136.5ms before the driver, at parity with iroh's 1.68ms.
  Deterministic `turmoil` partition/repair simulation stays green; the
  socket-mux accepts handshakes on non-primary sockets.
- WS1b backend-neutral `rds-net` facade: unified `EndpointConfig`
  (backend, secret key, bind addr, relay, ALPNs) and `Backend` selector;
  `Endpoint`/`Connection`/`Incoming` wrappers dispatch to either
  substrate while sharing the same `noq` stream/error types.
  `rds-agent`, `rds-cli` and `rds-bench` no longer depend on `iroh`
  directly — `--backend iroh|noq` selects at bind time. Owned-backend
  `addr()` now advertises dialable candidates (loopback + kernel egress
  hint) instead of the unspecified bind address. New evidence: agent
  e2e runs on `Backend::Noq`, and `rds-bench --backend noq` produces
  same-harness parity numbers on the direct path (handshake p50 20.5ms
  vs iroh 22.6ms; ping p50 1.31ms vs 1.49ms; suite compare within
  tolerance). Relay paths correctly refuse on `noq` until WS2.
- WS1 owned transport backend (`rds-net::backends::noq`, behind the
  `transport-noq` feature): noq endpoint with our own RFC 7250
  raw-public-key TLS layer (`tls.rs` — Ed25519 SPKI certs, server-name
  `EndpointId` verification, mutual client certs) that is
  wire-compatible with the shipping iroh backend on `rds/0`; BLAKE3
  stateless-reset key; endpoint facade (`bind/connect/accept/addr/close`)
  mirroring the iroh surface; candidate pipeline (`policy.rs` — dedup,
  cap at multipath limit, `open_path_ensure` for extra candidates, QNT
  round trigger); transport config matching iroh defaults (multipath ×8,
  QNT addrs ×32, QUIC datagrams, path keepalive/idle). Integration tests
  prove noq↔noq loopback, QUIC datagrams, and cross-backend interop
  (iroh client ↔ noq server, both directions). `iroh` remains the
  default backend until C1 parity.
- WS0 measurement harness: new dev-only `rds-bench` crate — scenario
  runner (handshake/ping/transfer/multiconnect/relay-fallback/impaired)
  on real QUIC paths, deterministic seeded UDP impairment proxy
  (loss/delay/jitter/rate), JSON+markdown reports and suite-vs-suite
  drift comparison (`max(rel, floor)` rule, separate tail band).
  `scripts/checkpoint.sh` implements the gate machinery from
  `docs/implementation-plan.md`; gate `c0` passed with committed
  evidence under `docs/reports/` (baseline-iroh.md).
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
