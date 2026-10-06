# Capability and requirement matrix

matrix_version: 1

Single source of truth for what this build claims to do. States:

- `implemented` — code landed and covered by tests/CI on a supported
  platform. Does not imply production or WAN qualification.
- `experimental` — code landed, real but partially qualified; gates,
  features or runtime probes may refuse it.
- `stub` — API/wire shape reserved; probing returns "not available" and
  no bytes move.
- `unavailable` — no code behind the name; roadmap only.

Runtime prerequisites are recorded separately from state: they describe
what a host/session must provide for a non-stub row to actually work.
Tests in `crates/rds-cli` and `crates/rds-bench` enforce that every
`ServiceKind` variant and every bench lane name has exactly one row
below; `README.md` must link this file.

## Services (agent `Info` advertisement and grant scopes)

The data-plane services are gated by deployment policy — `role`, `services`
and `disabled_services` in [agent configuration](agent-configuration.md), or
`--role`/`--service`/`--no-service` flags. A disabled service is refused by
name before grant machinery runs, and `Info`/directory announcements list
only what is actually served. `ping`/`info` are the always-on control plane.

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| service:ping | implemented | peer on `--allow` list | `rds-net` tests, CI |
| service:info | implemented | peer on `--allow` list | `rds-cli` tests, CI |
| service:tcp | implemented | enabled + `--allow` + grant scope for tcp | `rds-ssh` e2e, CI |
| service:desktop | experimental | enabled + `desktop` build flag; usable capture backend; grant scope | capture→encode→decode→stats contract tests |
| service:desktop-managed | implemented | manager-owned remote session relays encoded frames over local IPC v5; viewer decodes via `RelayDecoder` (`desktop` build flag for real decode) | relay e2e + managed-channel tests; `rds desktop` defaults to it, `--direct` kept |
| service:sync | implemented | enabled + `--sync-dir` configured | `rds-sync` tests, resumable transfer tests |
| service:audio | stub | no codec; wire shape reserved in v2 | `ServiceKind::Audio` variant only |
| service:sync-read | implemented | grant scope on sync root; optional grant `sync_paths` subtree | grant/scope tests |
| service:sync-write | implemented | grant scope on sync root; optional grant `sync_paths` subtree | grant/scope tests |
| service:desktop-view | implemented | grant scope; usable capture backend | scope tests; headless path only |
| service:desktop-control | experimental | desktop-view + control scope; X11 input sink | input contract tests; real-session injection unqualified |

## Transports

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| transport:iroh | implemented | default backend | CI both platforms |
| transport:noq | experimental | build `transport-noq` feature; `--backend noq` | bench + CI feature lane; WAN unqualified |
| transport:owned-relay | experimental | `rds-relay` deployment; `--owned-relay` pins | relay runtime tests; parity/migration open (W3) |

## Desktop backends (`rds-desktop` probe order)

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| desktop-backend:x11 | experimental | `x11` feature; X11 display socket; x11rb connect ok | xvfb contract tests; CI x11 lane |
| desktop-backend:image-copy | stub | ext-image-copy-capture-v1 compositor; backend unwritten | `probe()` returns `Ok(None)` |
| desktop-backend:kms | stub | DRM/KMS seat; backend unwritten | `probe()` returns `Ok(None)` |
| desktop-backend:pipewire | stub | XDG portal consent; backend unwritten | `probe()` returns `Ok(None)` |
| desktop-backend:screencapturekit | stub | macOS 12.3+, screen-record permission; backend unwritten | `probe()` returns `Ok(None)` |
| desktop-backend:dxgi | unavailable | Windows host; no module | roadmap only |

## Codecs and presentation

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| codec:openh264 | implemented | none (software) | encode/decode tests |
| codec:vaapi | stub | libva driver; unwritten | doc-only module |
| codec:vulkan-video | stub | ash `vk::Video*`; unwritten | doc-only module |
| codec:videotoolbox | stub | macOS; unwritten | doc-only module |
| render:wgpu | experimental | `viewer` build feature; native window and usable GPU surface | native window, bounded newest-frame presentation; [viewer contract](native-viewer.md) |

## Platforms

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| platform:linux-x86_64 | implemented | — | CI ubuntu lane |
| platform:macos-arm64 | experimental | native viewer; capture/input backends still stubbed; signing/notarization open | CI macos lane compiles+tests, Metal viewer qualification |

## Discovery, policy, observability

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| discovery:directory | experimental | `--directory`, `--directory-allow` publishers; HTTPS or explicit http | discovery tests; WAN unqualified |
| policy:grants-v2 | implemented | `--issuer` + grant files; `--revocations-key` for managed mode | grant/lease tests |
| policy:grants-v3 | implemented | same surface; adds `tenant`/`policy_revision` claims and `sync_paths` scope; agents pin via `--tenant`/`--policy-min-revision` | grant unit tests + e2e binding/scope suite; v2 payloads still verify |
| policy:gds-issuance | unavailable | estate GDS service wiring; not in this module | roadmap; `session renew` is manual-file only |
| observability:export | experimental | `RDS_LOG_FORMAT=json`; Vector/OpenObserve pipeline | fixture pipeline, `docs/observability.md` |

## Measurement lanes (`rds-bench` report scenario names)

| Capability | State | Runtime prerequisites | Evidence |
|---|---|---|---|
| measure:handshake | implemented | loopback agent | `docs/reports/` |
| measure:ping | implemented | loopback agent | `docs/reports/` |
| measure:transfer-receiver-ack-v1 | implemented | loopback agent + verified-receipt TCP target | `docs/reports/` |
| measure:multiconnect | implemented | impairment profile | `docs/reports/` |
| measure:relay-fallback | implemented | local iroh relay | `docs/reports/` |
| measure:impaired | implemented | impairment profile | `docs/reports/` |
| measure:resolve-connect | implemented | local registry fixture | `docs/reports/` |
| measure:migration | implemented | two local owned relays (noq); iroh lane reports SKIPPED | `docs/reports/` |
| measure:calibration | implemented | rate-cap impairment below the loopback ceiling; noq socket pacing or iroh proxy | `docs/reports/` |
| measure:recovery | implemented | live socket impairment (noq); iroh lane reports SKIPPED | `docs/reports/` |

Notes:

- `service:desktop` advertised by `Info` means the flag is on; a session
  still fails closed when no capture backend probes usable. X11 init
  failure is an explicit `Err`, and the x11 CI lane fails when the
  feature cannot initialize.
- `service:desktop-managed` is the local-IPC path: the manager forwards
  encoded frames and controls without decoding, so it works on headless
  manager builds; the *remote* still needs a capture backend, and the
  viewer build needs `rds-desktop/x11` for real decode.
- Historical `transfer` reports predate receiver-verified timing. The current
  verified lane is `transfer-receiver-ack-v1`; older sender-finish reports remain
  incomparable under [the benchmark contract](benchmark-transfer.md).
