# remote-device-sync architecture

Remote access for GDS devices: SSH reachability and remote desktop sessions
between any two enrolled devices, assisted by a GDS-operated server.
Design goals, in order: **minimum interactive latency**, **maximum
connection stability**, defense in depth, no inbound firewall changes.

Status: v0.1 foundation. This document records the protocol and stack
research and the decisions that fall out of it. The deeper second-pass
research — iroh 1.2/noq internals (multipath, path selectors, hooks),
capture/codec/input crate matrix, GDS server composition, and the
updated build order — lives in [research.md](research.md).

## Research summary

### Connectivity models surveyed

| Approach | Latency | Stability | Notes |
| --- | --- | --- | --- |
| Direct SSH to public IP | lowest | poor behind NAT | needs inbound ports; scanner exposure |
| Central relay only (RustDesk `hbbr`, Cloudflare Tunnel) | +1 RTT hop | high | always works, adds hop & server trust |
| P2P w/ relay fallback (Tailscale DERP, RustDesk hbbs+hbbr) | ~direct | high | >90% direct-path success reported by Tailscale |
| Mesh VPN (WireGuard/ Tailscale, Nebula, Netbird) | ~direct | high | solves reachability, not desktop |

Facts that shaped the design:

- **Tailscale** (DERP relays + STUN + in-band disco) reports direct-path
  success "well north of 90%". Connections *start* on the relay and migrate
  to a direct path — the relay is also the signaling channel. This is the
  model to copy: relay-first for instant connectivity, upgrade to direct.
- **RustDesk** splits the broker into `hbbs` (rendezvous/punch coordinator)
  and `hbbr` (relay). Same shape, older transport (TCP/UDP custom).
- **Cloudflare Tunnel** (`cloudflared`) is outbound-only and stable, but
  terminates at the CF edge, adds a permanent extra hop, couples the session
  to a third-party daemon lifecycle, and is a poor fit for interactive
  desktop latency. Decision: not a data path. Optionally documented later
  as a fallback reachability story for SSH.
- **Nebula lighthouses / Netbird** confirm the same pattern: a small public
  rendezvous host + P2P overlay. Our equivalent is the GDS server.

### Desktop streaming surveyed

| Stack | LAN latency | Notes |
| --- | --- | --- |
| Parsec | ~7–10 ms | closed source, managed broker |
| Sunshine+Moonlight | ~8–15 ms | NVENC/AMF/VT hw encode; best-in-class quality; GameStream protocol |
| RustDesk | ~18–30 ms | software encoders by default |
| RDP (AVC420/444) | ~15–25 ms | universal clients; bitmap pipeline |
| VNC | 30–100 ms+ | fallback tier only |

Encoding is the latency budget line item that matters: hardware encoders
(NVENC/VA-API/VideoToolbox/MediaFoundation) add ~5–15 ms; software H.264
(OpenH264, ~8–10 ms per 1080p frame with asm) is acceptable for v1; AV1
(rav1e, pure Rust) is an option where CPU headroom exists.

### Rust crate findings

- **`iroh`** (Apache-2.0/MIT): QUIC endpoints dialed by `EndpointId`
  (Ed25519 public key). Hole punching coordinated over the home relay,
  relay fallback, stream priorities, QUIC datagrams, multipath on the
  `noq` QUIC fork. `iroh-relay` is a self-hostable relay binary/library.
  This is the substrate: it is exactly the rendezvous+relay+P2P pattern
  above, production-maintained, and the endpoint key is a stable identity.
- **Media transport pattern**: iroh maintainers recommend *not* using QUIC
  datagrams for realtime media — datagrams still engage congestion control.
  The MoQ pattern is one uni-directional stream per frame: reset stale
  frame streams and prioritize the newest via `SendStream::set_priority`.
- **`webrtc-rs`**: v0.20 Sans-I/O rewrite is new; media-side gaps remain
  (no jitter buffer interceptor, no FEC, incomplete congestion control).
  Not selected for v1 media; revisit if a browser client appears.
- **`ironrdp-server`**: real RDP-server skeleton (TLS, FastPath input,
  bitmap updates, DVCs). Kept as a future interop frontend so stock RDP
  clients could attach to an agent.
- **Capture**: Wayland mandates `xdg-desktop-portal` ScreenCast + PipeWire
  (DMA-BUF zero-copy where the compositor offers it; `ashpd`/`lamco-*`
  crates, `pipewire` crate). X11: `x11rb` MIT-SHM `GetImage` polling +
  XFixes cursor + XDamage. macOS: ScreenCaptureKit. Windows: DXGI
  Desktop Duplication / Windows.Graphics.Capture.
- **Input**: Wayland RemoteDesktop portal (libei) via `ashpd`; X11 XTEST
  via `x11rb`; `uinput` as privileged fallback; Win `SendInput`,
  macOS `CGEvent`.
- **SSH**: do not implement SSH. Forward TCP to the host `sshd` over a QUIC
  stream — the user's ssh client keeps its own auth, keys, agent.

## Architecture

```text
┌──────────┐   direct QUIC (hole-punched)   ┌──────────┐
│  rds cli │◄──────────────────────────────►│ rds-agent│
│ (viewer, │   or relayed via the relay     │ (daemon, │
│  ssh -L) │◄────────────►┌──────────┐◄────►│  host)   │
└──────────┘              │rds-server│      └────┬─────┘
                          │ relay +  │           │
                          │ directory│      ┌────┴─────┐
                          │ (GDS)    │      │ sshd :22 │
                          └──────────┘      │ desktop  │
                                            └──────────┘
```

## Crate map and dependency direction

Dependencies point strictly downward; no cycles, no sideways deps at the
same layer. `docs/conventions.md` holds the enforceable rules.

```text
                     rds-core           types, framing, tokens — leaf
                       │  │
        ┌──────────────┘  └───────────────┐
        ▼                               ▼
  rds-discovery                     rds-net
  signed EndpointRecord,            transports: backends::iroh (now)
  stores (mem/file), GDS bridge         backends::noq (ours, §9)
        │                               │
        └──────────────┬────────────────┘
                       ▼
   ┌──────────┬──────────────┬───────────┐
   rds-relay  rds-desktop    rds-audio    rds-sync
   proto+     capture/codec/ Opus paths   FastCDC+BLAKE3
   iroh shim  input/render   (scaffold)   manifests/delta
   └──────────┴──────────────┴───────────┘
                       ▼
        ┌──────────────────────────────┐
        ▼                              ▼
   rds-agent (daemon)            rds-cli (operator)
                       ▼
   rds-server — GDS services host: relay + discovery + registry
```

- **`rds-server`** — runs on the GDS services host: the packet relay and
  the signed-record discovery directory; later the registry bridge into
  estate state, presence and audit. Sees only encrypted traffic.
- **`rds-agent`** — daemon on each controlled device. Binds the endpoint
  (Ed25519 identity persisted), connects to its home relay, accepts
  `rds/0` connections, serves streams to an allowlist of peers.
- **`rds`** — operator CLI. `rds id`, `rds ticket`, `rds ping`, `rds ssh`,
  `rds forward`, `rds desktop` (feature-gated); `rds send/recv` planned
  on the sync engine.

### Identity and authorization

- Network identity: the endpoint's Ed25519 key (`EndpointId`). QUIC-TLS
  authentication is built on it — connections are mutually authenticated
  by key, not by password.
- **Membership**: the agent serves only peers on its `allow` list of
  `EndpointId`s, checked at handshake completion.
- **Capability grants** (WS4): when `policy.issuers` is non-empty, the
  peer must additionally open `StreamHello::Authz` as the connection's
  first stream, presenting a grant signed by a trusted estate issuer.
  Every service stream raced ahead of the grant is refused; after
  verification each stream is scope-checked (`services`, `tcp_ports`,
  `displays`, `max_bps`). Grants are short-lived (`grant_max_ttl`),
  non-replayable across concurrent connections (`active_grants`), and
  revocable: the directory serves an estate-signed `SignedRevocations`
  snapshot at `GET /v1/revocations`, agents poll it into their denylist,
  and a revoked or expired grant closes its live connection.
- GDS binding: a signed device record ties `device_id` ↔ `EndpointId`
  and is distributed through the estate/device registry, so
  `rds ssh nddev-amsterdam` resolves keys from GDS state instead of
  pasted tickets.

### Stream protocol (`ALPN = rds/0`)

Every stream opens with a length-prefixed postcard `StreamHello`:

| Service | Direction | Payload |
| --- | --- | --- |
| `Authz` | bi | capability grant (first stream in grant mode) |
| `Ping` | bi | nonce echo for RTT |
| `Info` | bi | agent version, services, displays |
| `TcpConnect { host, port }` | bi | raw byte splice (ssh = `127.0.0.1:22`) |
| `Desktop` | bi + uni | hello/capabilities; input events client→server; one uni stream per video frame server→client |
| `Sync` | bi + uni | manifest offer/request; chunk pull on dedicated streams (WS6) |

Desktop media: capture → BGRA→I420 → H.264 (OpenH264 baseline, no B-frames;
hw encoders behind a trait) → per-frame uni stream with
`{seq, pts_ms, keyframe}` header; the receiver resets streams overtaken by
newer frames; `RequestIdr`/`SetBitrate` control messages; input events on
the control stream. This yields decode-what-survives behavior without a
custom UDP stack.

### Stability measures

- Relay-first connect (works on any egress-only network), in-band
  hole-punch upgrade — both handled by iroh.
- QUIC connection migration survives NAT rebinding/Wi-Fi↔LTE moves.
- Agent reconnects to relay with backoff; CLI can pin `--relay`.
- Frame-stream reset semantics bound worst-case latency under loss.

## Milestones

1. **v0.1 (this)**: workspace, rendezvous/relay, auth allowlist, `ping`,
   `ssh`/TCP forward E2E, desktop pipeline traits + X11 capture/encode/
   input behind the `desktop` feature, architecture doc.
2. **v0.2**: GDS discovery + authz — `iroh-dns-server` on gds-services,
   `EndpointHooks` allowlist, signed `device_id`↔`EndpointId` registry,
   `rds ssh <device-name>`; damage-driven (VFR) capture replacing the
   fixed-fps loop; wgpu client render.
3. **v0.3**: hardware encode (`cros-codecs` VA-API/V4L2, `gpu-video`
   Vulkan path), `wdotool-core`/portal-EIS input, audio (opus),
   clipboard; `rds send/recv` via iroh-blobs, registry replication via
   iroh-docs.
4. **v0.4**: multi-relay failover + custom `PathSelector`, adaptive
   bitrate from path congestion state, AV1 tier, RDP frontend via
   `ironrdp-server`, browser client via WebRTC if needed.

## Non-goals for v0.1

- No SSH protocol implementation (TCP forward only).
- No unattended access control model beyond the EndpointId allowlist.
- No file transfer, audio, multi-monitor.
