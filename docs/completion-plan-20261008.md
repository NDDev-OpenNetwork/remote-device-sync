# System completion plan — 2026-10-08

This plan is the executable follow-up to `continuation-plan.md`. It was
rechecked against the current `main` tree (`03410ee`) and the capability matrix
before implementation. A capability changes state only after its code,
wire/authz path, platform probe, tests, and installed qualification all pass.

## Current truth

- Connectivity, signed discovery, grants v2/v3, managed local sessions,
  single-file resumable sync, Linux X11 desktop, and the default Iroh lane are
  implemented or experimental as recorded in `capability-matrix.md`.
- Audio now has a bounded libopus packet/jitter core, but no source, sink,
  agent admission or viewer path. The service capability remains `stub`.
- Linux Wayland image-copy/KMS/portal capture and macOS ScreenCaptureKit,
  VideoToolbox and CGEvent are explicit stubs. X11 is the only capture/input
  backend that moves pixels on a supported device.
- Sync is intentionally single-file. Its journal and confinement are the
  safety foundation; recursive/two-way manifests, tombstones, conflict policy,
  watch/reconcile and journal GC are not implemented.
- Noq and owned relay have strong in-process evidence but remain experimental
  until the topology, interface-change, UDP-blocked and parity matrix is run on
  real supported devices.
- Automatic GDS enrollment, grant issuance/renewal/revocation and applied
  policy acknowledgements are outside this public module and must be integrated
  through the estate authority rather than hand-edited grant files.

## Ordered workstreams and gates

### A — Audio core and wire path

1. Keep `rds-core` audio tags stable; add a bounded `rds-audio` core with
   validated Opus formats, fixed 20 ms default frames, packet size limits,
   sequence numbers, capture timestamps, decoder PLC and a bounded reorder/
   jitter buffer.
2. Add audio service admission and grant scope only after a real source/sink
   abstraction is injectable in tests. No service advertisement from a stub.
3. Add Linux PipeWire/CPAL and macOS CoreAudio adapters behind platform
   features. Source callbacks must never block on network or disk; a bounded
   queue drops oldest audio with counters and preserves control priority.
4. Add audio uni-stream framing, loss/late/duplicate handling, clock drift
   observation and a sink adapter. Keep microphone permission separate from
   desktop-view permission.

Gate A: loopback encode/decode, malformed packet rejection, deterministic PLC,
reordering/loss matrix, bounded memory, and an injected source/sink end-to-end
session. Only then change `service:audio` from `stub`.

### B — Recursive sync foundation

1. Add a directory manifest format with typed file/dir/symlink metadata,
   normalized relative paths, deterministic ordering and explicit unsupported
   attribute states. Never follow symlinks while scanning. **Model and the
   bounded no-follow scanner landed in `rds-directory-scanner-20261008`; the
   recursive wire/reconcile layer is still open.**
2. Add snapshot/reconcile operations on top of the existing journal, with
   tombstones, rename detection by stable content identity, bounded entries and
   cancellation checkpoints.
3. Define one-way mirror first (dry-run/change preview, delete policy and
   resumable operation IDs), then two-way conflict records and metadata policy.
4. Add journal quotas/GC with inode ownership proofs; cleanup may remove only
   complete owned records and never guesses from a filename.

Gate B: seeded Linux/macOS fixtures, crash-at-every-commit boundary, disk full,
symlink races, offline event repair, conflict preservation and exact digest
convergence. Single-file protocol compatibility remains unchanged.

### C — Native platform backends

1. macOS: ScreenCaptureKit stream output with complete-frame filtering,
   IOSurface-backed frames and bounded queue depth; VideoToolbox encode/decode;
   CGEvent input with TCC denial/regrant states; launchd/user-session identity.
2. Wayland: portal RemoteDesktop + ScreenCast session, PipeWire stream
   selection by `pipewire-serial`, EIS input, restore-token lifecycle, then
   ext-image-copy-capture-v1 where compositor permission exists. KMS is a
   separately privileged unattended profile, never an implicit fallback.
3. Keep backend probe order and capability reporting truthful: unavailable,
   denied and runtime-failed are different states. No shelling out to capture
   utilities.

Gate C: real macOS arm64 and at least two Wayland compositor profiles, monitor
selection/resize, lock/unlock/suspend, permission denial/regrant, input release
on disconnect, and exact installed artifact/signing identity.

### D — Transport and policy completion

1. Complete Noq/Iroh parity matrix: interface changes, NAT rebinding, suspend,
   UDP-blocked TLS fallback, relay death, directory outage, path accounting,
   churn hysteresis and service-aware recovery.
2. Integrate GDS enrollment and revisioned policy acknowledgement through the
   estate control plane. Public code accepts signed policy snapshots and reports
   stale/offline state; it never owns private tenant facts.
3. Add negotiated resource/QoS classes for control, terminal, desktop, audio
   and bulk sync. Prove fairness under mixed load with RSS/FD/task ceilings.

Gate D: identical scenario definitions on Iroh and Noq, explicit NOT-RUN rows
for unavailable topologies, and no owned-backend promotion without parity.

### E — Release and operations

1. Make observability reports machine-produced with source/features/toolchain,
   backend, topology, sample counts, failures and skips.
2. Add secured redacted diagnostic bundles and applied-policy status.
3. Qualify signed/notarized macOS and Linux artifacts, atomic update/drain,
   rollback, launchd/systemd health and compatibility with older peers.

Gate E: clean-machine install, exact running digest, rollback after interruption,
all mandatory acceptance rows, and synchronized public/estate/controller state.

## Research decisions used by this plan

- ScreenCaptureKit delivers `CMSampleBuffer` output and IOSurface-backed video;
  discard non-complete frames and keep capture queues bounded. See Apple’s
  [SCStream documentation](https://developer.apple.com/documentation/screencapturekit/scstream)
  and [capture sample](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos).
- XDG ScreenCast persistence uses single-use restore tokens; combined remote
  desktop persistence belongs to the RemoteDesktop portal. Use EIS for input
  rather than legacy D-Bus notify calls. See the
  [ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
  and [RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
  contracts.
- Wayland image-copy capture is still a testing-stage protocol; negotiate
  compositor buffer constraints and treat `failed` as a visible capability
  result. See the [protocol definition](https://wayland.app/protocols/ext-image-copy-capture-v1).
- Opus uses standard 2.5/5/10/20/40/60 ms frame sizes; custom modes add delay
  and reduce interoperability. The core uses 20 ms by default and bounded
  packet validation, based on the [Opus API guidance](https://opus-codec.org/docs/opus_api-1.6.pdf)
  and [RFC 6716](https://www.rfc-editor.org/rfc/rfc6716).

Each workstream lands as a signed conventional-commit PR, with its own report
and checkpoint. After every merge: re-run public checks, refresh estate pin if
needed, qualify exact artifacts, refresh controller snapshot in preserve mode,
validate memories, and verify local/origin/controller/runtime parity.
