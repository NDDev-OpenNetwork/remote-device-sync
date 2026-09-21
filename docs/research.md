# Deep research: remote + sync, all-Rust, minimum latency

Second-pass research, deeper than `architecture.md`. Goal: the lowest-latency,
fully-Rust remote access + sync system serviced by the GDS server. This
document separates verified facts (crate versions, APIs read in vendored
sources, upstream docs) from measured folklore, and ends with concrete
decisions and a build order.

## 0. Executive summary — what changed vs v0.1

| Area | v0.1 assumption | Deep-research correction |
| --- | --- | --- |
| QUIC impl | "quinn via iroh" | iroh 1.2 runs on **noq** — a real fork with **QUIC Multipath + QNT + QAD merged**. Relay and direct are *simultaneous first-class paths* with per-path RTT/congestion, not magic-socket trickery. |
| Congestion | "default cubic" | noq ships **BBRv3** (`noq-proto::congestion::bbr3`). quinn's own BBR is stale; noq's is the current implementation. Exposed via `transport_config`. |
| Path policy | "accept iroh default" | `Builder::path_selector(Arc<dyn PathSelector>)` — we can bias selection (e.g. hold relay for input stream while video migrates to direct). |
| Authz point | allowlist at first stream | `EndpointHooks::after_handshake` — reject at TLS completion, before any stream opens. Stronger boundary, less code in service path. |
| Discovery | tickets by hand | `AddressLookup` trait is pluggable; **self-hosted `iroh-dns-server` 1.2.0** (pkarr PUT + DNS/DoH GET) is the GDS discovery component — signed, key-verified records, zero custom code. Or implement `AddressLookup` over the GDS API. |
| Wayland capture | "portal+PipeWire only" | Three tiers now: **ext-image-copy-capture-v1** (merged protocol, damage+dmabuf, privileged clients — no consent dialog), **libdrmtap 0.5.7** (kernel DRM/KMS scanout — works at login screen/headless, RustDesk's own answer to unattended Wayland), portal+PipeWire as the consented path. |
| Wayland input | "portal RemoteDesktop" | **`wdotool-core` 0.5.3** already wraps all four backends: libei/EIS via portal, wlr virtual-kb/pointer, KWin scripting, GNOME shell ext. Reuse, don't rebuild. |
| Encode hw | "VA-API via ffmpeg" | **Two pure-Rust paths**: `cros-codecs` (VA-API H264/VP9/AV1 enc + V4L2; fork `cros-codecs-generic-vaapi` accepts plain libva surfaces — PipeWire dmabuf fits) and **`gpu-video` 0.4.0** (Vulkan Video, H264 enc+dec to/from `wgpu::Texture` — vendor-agnostic GPU path). |
| Reference impl | none known | **`nchapman/tether`** — low-latency remote desktop in Rust, PipeWire→VAAPI/NVENC→QUIC→hw decode→wgpu, plus ScreenCaptureKit/VideoToolbox and DXGI/D3D11 backends. Our architecture is validated by an existing codebase; mine it for buffer-pool/modifier details. |
| Sync | undefined | `iroh-blobs 0.103` + `iroh-docs 0.101` + `iroh-gossip 0.101` all target `iroh ^1` — content-addressed file sync, CRDT device registry, presence gossip. `sendme`/`dumbpipe` are the reference CLIs. |

## 1. Transport: iroh 1.2 / noq internals

### 1.1 Multipath is real now

iroh ≥0.96 runs on **noq** (n0's QUIC fork, `noq`/`noq-proto` 1.3.0 in our tree).
Merged draft extensions: **QUIC Multipath**, **QNT** (QUIC NAT traversal —
REACH_OUT/PUNCH_* frames inside QUIC), **QAD** (address discovery). Source:
`iroh.computer/blog/noq-announcement`, issue `n0-computer/iroh#3276` (merged).

What this buys us concretely:

- **Relay is a path, not a socket**: the connection opens on the relay path
  instantly (no hole-punch wait), then direct IPv4/IPv6 paths are *added*
  in-band via `ADD_ADDRESS`/`REACH_OUT`. Per-path state is visible:
  `Connection::rtt(PathId)`, `congestion_state(PathId)`, `paths` watcher.
- **Correct congestion accounting**: each path gets its own congestion
  controller state — the exact defect that pushed n0 off quinn.
- **Migration = path loss, not reconnect**: Wi-Fi→LTE or NAT rebinding
  degrades to another live path; streams stay open.

Measured upstream: hole-punch setup costs ~3–35 KB of relay traffic and
direct path wins in the well-documented >90% of cases (Tailscale reports
"well north of 90%"; symmetric cloud NATs — AWS/Azure default egress — are
the main failure mode; see §7 relay sizing).

### 1.2 Knobs iroh 1.2 exposes (verified in vendored source)

`Endpoint::builder(preset)`:

- `.transport_config(QuicTransportConfig)` → noq `TransportConfig`:
  `congestion_controller_factory` (cubic / new_reno / **bbr3**),
  `AckFrequencyConfig`, `initial_mtu`, `max_idle_timeout`,
  `default_path_keep_alive_interval`, `default_path_max_idle_timeout`.
- `.path_selector(Arc<dyn PathSelector>)` — default is
  `BiasedRttPathSelector`; we can implement a custom selector, e.g.
  "prefer direct; pin control stream to whichever path is alive; prefer
  lowest-jitter not lowest-RTT for video".
- `.address_lookup(impl AddressLookupBuilder)` — pluggable discovery
  (§3). `presets::Minimal` adds nothing; `presets::N0` adds n0's
  pkarr+DNS+relay.
- `.hooks(impl EndpointHooks)` — `before_connect` (pre-packet) and
  `after_handshake` (post-TLS, knows remote `EndpointId`+ALPN; returning
  `Reject` kills the conn before streams). **Move the agent allowlist
  here.** Also usable for audit logging at connection granularity.
- `.relay_mode(RelayMode::Custom(RelayMap::custom(iter)))` — multiple
  relay URLs; `Endpoint::home_relay_status()` watcher reports
  per-relay connectivity.
- `.portmapper_config`, `.net_report_config`, `.proxy_from_env`,
  `.external_addr`, `.keylog`, `.max_tls_tickets` (0-RTT control).
- `Endpoint::metrics() -> &EndpointMetrics` + `iroh-metrics` Prometheus
  exporter — `send_relay`/`send_ipv4`/`connection_became_direct` etc.
- `Connection::send_datagram`/`read_datagram`, `max_datagram_size`,
  `datagram_send_buffer_space` — QUIC DATAGRAM extension is available
  (see §4.5 for why video still shouldn't use it; input/heartbeats can).
- 0-RTT: `Connecting::into_0rtt` exists — session resumption for
  reconnect-heavy UX is available, gated by `max_tls_tickets`.

### 1.3 Wire format

postcard+length-prefix for control is fine (messages ≤64 KB, cold path).
Media stays raw bitstream — no serde in the hot path. If control ever
gets hot, `rkyv` is the zero-copy upgrade; not needed now.

## 2. Desktop media pipeline

### 2.1 Latency budget (synthesis of scrcpy/Sunshine/Parsec data)

```
capture ──► convert ──► encode ──► send ──► [net] ──► decode ──► present
1–4 ms      0–3 ms      2–10 ms    ~1 ms     RTT/2    2–8 ms     0–16 ms
```

- scrcpy measured: end-to-end ~33–70 ms on LAN; manual packetization
  alone saved a full frame; "display newest, drop late" is the rule.
- Sunshine: `llhp` NVENC preset + CBR; **VFR — encode only when content
  changes** (damage-driven); SPS `max_dec_frame_buffering` restriction
  gave ~100 ms on some hw decoders — bitstream hygiene matters.
- Parsec (BUD, custom UDP+DTLS): latency > framerate > quality;
  zero-copy GPU pipeline; dynamic bitrate driven by network feedback.
- Target envelope: **LAN ≤20 ms glass-to-glass, WAN ≤50–80 ms** at
  1080p60. Achievable only with damage-driven capture + hw encode +
  no queue build-up anywhere.

### 2.2 Capture matrix (all reachable from Rust)

| Path | Crate | Zero-copy | Consent | Notes |
| --- | --- | --- | --- | --- |
| Wayland — direct protocol | `wayland-client` + `wayland-protocols` (`ext-image-copy-capture-v1`, staging) | dmabuf | none (privileged clients) | Merged ext protocol; per-frame damage; cursor sessions; wlroots/mir + smithay servers already implement. Best engineering surface. |
| Wayland — kernel scanout | `libdrmtap` 0.5.7 | dmabuf fd | none | Works at GDM/login, headless, no compositor. Needs DRM-master or `CAP_SYS_ADMIN` (helper ships setcap flow). RustDesk PR #15420 adopted this exact approach for unattended Wayland. Our agent is a system service — fits. |
| Wayland — consented | `ashpd` RemoteDesktop/ScreenCast → `pipewire` 0.10 / `lamco-pipewire` 0.7 (damage, cursor, multi-monitor, dmabuf) | dmabuf | portal dialog | Required on GNOME/KDE for unprivileged apps; fine for attended sessions. `lamco-pipewire` already implements damage tracking + adaptive bitrate hooks. |
| wlroots fast path | `libwayshot` 0.9 / `wayland-protocols-wlr` (wlr-screencopy) | shm/dmabuf | none | Where wlr compositors live. |
| X11 | `x11rb` MIT-SHM `GetImage` + XDamage + XFixes cursor | shm | none | Current baseline; polling, per-screen. |
| Windows | DXGI Desktop Duplication / Windows.Graphics.Capture (`windows` crate) | texture | none (DXGI) | Tether's host does DXGI→D3D11→vendor enc. |
| macOS | ScreenCaptureKit (`screencapturekit` crates) | IOSurface | TCC | SCK + VideoToolbox, the only supported route. |

Fallback order on Linux agent: **ext-image-copy-capture → libdrmtap →
portal+PipeWire → X11**. Capability-probe at runtime, log the demotion
reason (mirrors RustDesk's strategy).

### 2.3 Encode

| Backend | Crate | Notes |
| --- | --- | --- |
| Vulkan Video | `gpu-video` 0.4.0 | H.264 enc+dec, H.265 enc, AV1 enc WIP. In/out as `wgpu::Texture` — pairs with dmabuf import for a GPU-resident pipeline. Vendor-agnostic (NV/AMD/Intel). Young; verify per-GPU. |
| VA-API / V4L2 | `cros-codecs` (+`-generic-vaapi` fork) | ChromeOS lineage, safe Rust. H264/VP9/AV1 enc, accepts external surfaces in the fork. Linux-native, battle-tested in crosvm. |
| Software floor | `openh264` 0.9 | Already integrated. CBR + `CameraVideoRealTime` + skip-frames + `force_intra_frame` = correct latency profile. ~8–10 ms/1080p frame. |
| AV1 sw | `rav1e` speed 10 + `dav1d` decode | Realtime-capable but lookahead costs latency; keep as quality tier, not default. |
| Kitchen sink | `ffmpeg-next` | Universal hw fallback (NVENC/AMF/QSV/VT/MF) — RustDesk's actual hwcodec path. Last resort; heavyweight dep. |

Settings that matter (from Sunshine/scrcpy evidence): constrained
baseline, no B-frames, IDR on request + on resize, CBR with adaptive
ceiling, `max_dec_frame_buffering` restricted in SPS, bitrate paced to
network feedback (§4.3).

### 2.4 Decode + render (client)

- `gpu-video` decodes H.264 straight into `wgpu::Texture` → blit to
  surface. For sw path: openh264 → `queue.write_texture` BGRA, or
  `pixels`/`softbuffer` fallback. lumina-video's ZERO-COPY.md documents
  dmabuf→Vulkan import for NV12 planes (multi-plane import + shader
  YUV→RGB) — the pattern to follow when hw decode lands.
- Render policy: **always display the newest fully-decoded frame**;
  never queue. scrcpy's 12 ms decode→present average is the bar.

### 2.5 On-wire media shape

Keep the v0.1 decision — MoQ-pattern per-frame uni streams:

- iroh maintainers advise *against* QUIC datagrams for realtime media:
  datagrams still pay congestion control. Streams give us per-frame
  reliability boundary + `set_priority` + cheap reset.
- Our extras: `AckFrequencyConfig` to thin ACK traffic; input events on
  the high-priority control bidi stream; cursor position can move to
  datagrams later (lossy is fine there).
- If a "watch-only broadcast to N viewers" mode ever appears, adopt
  `moq-lite`/`hang` (v0.17/0.20, pub/sub + CDN fan-out) — not worth the
  abstraction for 1:1 now.

### 2.6 Input injection

- Wayland all-backends: **`wdotool-core`** — portal libei/EIS, wlr
  virtual-kb/pointer, KWin, GNOME shell. Compositor-respecting.
- Portal direct: `ashpd` RemoteDesktop → `ConnectToEIS` fd → raw EIS
  protocol (portal drops out of the data path after handshake).
- X11: XTEST (implemented). Headless/privileged: `evdev`/`uinput`
  (`input-linux` or `evdev` crate) — for VMs and kiosks.
- Security note: input injection must require `desktop` service authz
  *and* map to a scoped seat — a compromised peer shouldn't inject into
  other sessions. Portal/EIS gives this for free on Wayland.

## 3. GDS server — "services everything"

The GDS services host carries four roles; three are off-the-shelf iroh
infrastructure, one is ours:

```
gds-services
├── iroh-dns-server 1.2.0    pkarr PUT /pkarr + DNS/DoH resolve
│                            (signed EndpointInfo records; endpoints
│                             self-publish, key-verified)
├── iroh-relay (rds-relay)   data relay + hole-punch coordination;
│                            AccessControl allowlist (already built)
├── rds-directory (ours)     device registry: signed device_id↔EndpointId
│                            records, ACL/allowlist distribution,
│                            enrollment, audit log
└── optional: iroh-docs/blobs seed  registry replication + file-sync
                                    seeding
```

Discovery integration choices (pick one at build time, trait is clean):

1. **Standard**: point `PkarrPublisher`/`DnsAddressLookup` at our
   `iroh-dns-server` instance (`presets::Minimal` + two
   `.address_lookup(...)` calls). Zero custom protocol, DNS ops story,
   interoperable with stock iroh clients.
2. **Custom**: implement `AddressLookup` {`publish`,`resolve`} over a GDS
   HTTPS endpoint — lets the registry carry ACL metadata (services,
   owner, expiry) in the same record, and gives one place for authz
   checks. `AddressLookup` is a 2-method trait; `publish` is
   fire-and-forget from a tokio task.

Recommendation: **start with (1)** — infra exists today; add (2) when
GDS wants registry-side policy. Both can coexist (`address_lookup` calls
compose into `AddressLookupServices`).

Authz beyond the allowlist, when needed: **biscuit-auth 6.0** — GDS
mints root-signed tokens (`right(device, "ssh")`), holder attenuates
offline (time-boxed delegation), agent verifies with the GDS public key.
Fits the "signed device record" direction without a session DB.

## 4. Reliability engineering

- **Multiple relays**: `RelayMap::custom([ams, nyc, …])`; endpoint keeps
  a home relay but paths can span several. Deploy relays near users;
  `home_relay_status()` watcher for health.
- **Reconnect**: QUIC migration covers network moves; for hard drops,
  `into_0rtt` + `max_tls_tickets` shortens re-handshake. Agent-side
  backoff: `backon` (already in tree).
- **Path pinning**: custom `PathSelector` can keep the SSH/control
  stream on the most stable path while video rides lowest-RTT —
  worth it because relay paths are stable-but-slower and direct paths
  can flap during re-punch.
- **Observability**: `endpoint.metrics()` + `iroh-metrics` → Prometheus;
  `net_report()` watcher (NAT status, relay RTT) for agent diagnostics;
  `qlog` support in noq for deep debugging.
- **Congestion**: default cubic is fine; evaluate `bbr3` for media
  (better bandwidth estimate under loss) — it's config, not code.
  quinn-line BBR1 is stale; noq's bbr3 is the current one.
- **FEC**: optional `reed-solomon-erasure` on the frame stream for
  lossy-WAN resilience; measure first — stream reset + IDR-on-loss may
  be enough at desktop bitrates.
- **Bitrate control**: Sunshine's lesson — pace encoder to *measured*
  path. Wire `Connection::congestion_state(path)`/RTT/loss into
  `SetBitrate` already in the protocol.

## 5. The "sync" half of remote-device-sync

iroh ecosystem gives the whole story on the same endpoint:

- **File sync / transfer**: `iroh-blobs 0.103` (blake3-verified,
  resumable; `sendme` is the reference CLI). Powers `rds send/recv`
  and bulk sync between enrolled devices.
- **Device registry replication**: `iroh-docs 0.101` — CRDT kv docs
  (set-reconciliation + gossip notifications). The GDS device registry
  can be a replicated doc: agents subscribe, get ACL updates live.
- **Presence**: `iroh-gossip 0.101` — HyParView/PlumTree broadcast;
  `device-online` beacons per estate topic.
- All three compose on `protocol::Router` with our `rds/0` ALPN on the
  same endpoint — one identity, one connection pool.

## 6. Rejected / deferred (with reasons)

| Option | Verdict |
| --- | --- |
| Cloudflare Tunnel as data plane | Adds fixed extra hop + third-party daemon in the datapath; kept only as optional SSH reachability integration. |
| RDP primary | `ironrdp-server` 0.13 is real but still a skeleton (bitmap updates, no GFX pipeline server-side); possible interop frontend later, never the main transport. |
| WebRTC (webrtc-rs 0.20 / str0m) | Both matured (0.20 Sans-I/O ships; str0m proven SFU). Still buys us ICE+TURN we already have via iroh, plus media complexity we don't need. Revisit only for a browser client. |
| moq-lite/hang now | Pub/sub+fan-out machinery is overkill for 1:1 desktop; our per-frame streams already implement the core idea. Adoption trigger: multi-viewer broadcast. |
| Raw quinn instead of iroh | Would mean re-implementing relay-as-path, hole punching, discovery, multipath. iroh 1.2 is the substrate; divergence is irrational. |
| Self-rolled crypto | Endpoint identity is already Ed25519-over-QUIC-TLS; pkarr records are signed. No new primitives. |

## 7. Risk register

- **Symmetric/cloud NAT**: AWS/Azure default egress = worst case; relays
  carry the load → size relay bandwidth for ~10% of fleet traffic
  (Tailscale's own guidance: give infra nodes public IPs to escape it).
- **Relay throughput**: upstream measured ~28→48 MiB/s after a Nagle
  fix on relayed streams; relay CPU is the scale ceiling. Multi-relay +
  metrics from day one.
- **Vulkan Video driver reality**: `gpu-video` is young; treat as
  experimental behind a probe, ship cros-codecs/VA-API first.
- **Wayland permission model**: portal consent is interactive — for
  *unattended* access the agent must be privileged (service + libdrmtap)
  or pre-consented via `persist_mode` portal restore tokens. This is a
  product decision, not a bug.
- **iroh-blobs API churn**: n0 flags blobs as pre-1.0; isolate behind
  our own `sync` facade so upgrades are mechanical.
- **Input injection abuse**: scope to seat/session; audit every
  control-stream event at debug level.

## 8. Build order (recommendation)

1. **Discovery+authz hardening**: `EndpointHooks::after_handshake`
   allowlist (replace stream-level check), `iroh-dns-server` on
   gds-services, agent publishes EndpointInfo; tickets stay as fallback.
2. **GDS directory service**: signed `device_id↔EndpointId` records,
   `rds ssh nddev-amsterdam` name resolution, audit log.
3. **Damage-driven capture**: replace fixed-fps loop with
   damage/change-triggered encode (Sunshine VFR) — biggest real latency
   + bandwidth win available.
4. **hw encode**: `cros-codecs` VA-API path behind probe; keep openh264
   fallback. Then `gpu-video` Vulkan path experiment.
5. **Client render**: wgpu surface + newest-frame-only policy.
6. **Input**: `wdotool-core` integration; portal EIS path.
7. **Sync**: `rds send/recv` via iroh-blobs; registry doc via iroh-docs.
8. **Resilience**: multi-relay map, custom `PathSelector`, reconnect
   tests, loss/jitter harness (`netem`), p50/p99 glass-to-glass bench.

## Sources (selected)

- iroh: noq announcement & multipath write-ups (iroh.computer/blog),
  `iroh` 1.2.0 vendored source (`endpoint.rs`, `address_lookup.rs`,
  `socket/remote_map.rs`, `socket/biased_rtt_path_selector.rs`),
  `noq-proto` 1.3.0 `congestion/bbr3`, iroh issue #3876 (hole-punch
  relay traffic), iroh PR nagle fix (#3995, 28→48 MiB/s relayed).
- Media: moq-dev/moq (moq-lite 0.17, hang 0.20), scrcpy develop.md +
  PR #646 latency measurements, Sunshine docs/issues (llhp/CBR, SPS
  buffering fix), Parsec BUD posts, `nchapman/tether`,
  `yuv418/lightvideo`, `lumina-video` ZERO-COPY.md.
- Capture/input: wayland.app ext-image-copy-capture-v1, Andri Yngvason's
  protocol design post, smithay impl, `libdrmtap` + RustDesk PR #15420,
  `lamco-pipewire`, `libwayshot`, `wdotool-core`, ashpd RemoteDesktop,
  libei/liboeffis docs.
- Codecs: `cros-codecs` (+generic-vaapi fork), `gpu-video`/`vk-video`,
  `openh264`, rav1e/dav1d project docs.
- Server-side: `iroh-dns-server` 1.2.0 (pkarr/DNS/DoH),
  `AddressLookup` trait, biscuit-auth 6.0, Tailscale NAT-traversal
  series + connection-types docs.
- Sync: iroh-blobs 0.103 / iroh-docs 0.101 / iroh-gossip 0.101 changelogs
  (all `iroh ^1` compatible), `sendme`/`dumbpipe`.
