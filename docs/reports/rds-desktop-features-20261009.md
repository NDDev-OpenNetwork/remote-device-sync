# Clipboard, monitors and concurrent windows — 2026-10-09

This increment serves Linux X11 desktops to the native macOS viewer. It adds
bounded bidirectional UTF-8 text clipboard, one shared monitor catalog and
explicit simultaneous device/display windows. Legacy desktop wire tags remain;
reverse clipboard negotiates V5. Implementation and review:
[PR #139](https://github.com/NDDev-OpenNetwork/remote-device-sync/pull/139).

## Native and transport evidence

| Qualification | Result | Boundary |
|---|---|---|
| Dedicated Xvfb, two X roots and two RandR regions | Five native library tests passed | Known pixels, pointer offsets, removed-monitor refusal; UTF-8/INCR both ways, repeated same-owner copies, empty text and no own-publication echo |
| Real isolated V5/Noq clipboard session | Included in the five native tests | Offer/request/chunks, stale offer rejection retaining the fresh offer, large INCR text and heartbeat while media output waits |
| Native AppKit named pasteboard | Four cases passed | Empty, Unicode, large text and oversized read/write refusal on the main thread; no mutation of the user's general clipboard |
| Real local manager, two peers and two routes on one peer | Iroh and Noq passed | Closing one preserves other video, input ACKs and heartbeat; selected-session state cannot redirect an explicit route |
| Native viewer launch planning | Five tests passed | Explicit destinations, monitor choice, eight-window ceiling, deduplication, conflicting-grant refusal and saved quality |
| Real Iroh standby/recovery fixtures | Three tests passed | Initial blocked standby, retired standby restoration and existing-stream blackhole recovery |

Reproduce using disposable identities and Xvfb, never an existing desktop:

```sh
xvfb-run -a -s "-noreset -screen 0 1280x720x24 -screen 1 640x480x24" \
  cargo test --locked -p rds-desktop --features x11 -- --ignored --test-threads=1
cargo run --locked -p rds-desktop --features viewer --example native_clipboard
cargo test --locked -p rds-client --features transport-noq --test multiple_desktops
cargo test --locked -p rds-cli --features desktop --bin rds-viewer
cargo test --locked -p rds-net --features transport-noq --lib actual_iroh_actor_
```

## Failure retention

The macOS job in [CI run 37907548073](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/37907548073)
failed the existing `frame_delivery_budget` recovered-delay assertion:
the observed encoder target was 2.8 Mbps instead of its initial 4 Mbps, with
zero producer deadline misses. Two complete local executions passed. The
fixture now reports its actual gated duration on failure; product thresholds
and the quality assertion are unchanged. Local success does not erase that
failure or establish its cause. Final PR checks remain independent evidence.

An earlier Iroh fixture waited for UDP standby while its setup selector did
not opt into standby maintenance. Setup now explicitly retains standby paths,
matching the blackhole assertion's precondition. The real stream recovery
assertion and bounded deadline are unchanged.

## Resource and privacy contracts

The clipboard worker wakes on XFixes/native socket readiness, with 4 Hz owner
polling only without XFixes. It runs only for permitted V5 or explicit legacy
paste. Eight native workers, one offer and one outgoing value per session,
1 MiB text limits, bounded queues and idle/total deadlines bound copying.
Viewer publication is main-thread-only, fenced by pasteboard change count.
Background windows cannot initiate replacement; view-only sessions cannot
read or publish clipboard. Payloads are redacted from diagnostics.

Each native window is a separate process with an explicit peer/display and
its own decoder/control/clipboard state. Children are reaped without making
one window's lifetime own the others. Default 30 FPS and saved Full HD apply.
Inventory reads no longer allocate a capture image; bounded native probes
run off the transport executor. These are structural resource improvements,
not measured CPU savings.

Logical RandR IDs preserve root IDs and remain stable under enumeration
reordering. Inventory, capture and input share them. Geometry is checked
before native I/O; removed/changed regions refuse the session. Logical
monitor names are not EDID hardware identities.

## Qualification boundaries

CI completion, installation and ordinary deployed interactive acceptance are
separate observations. No physical multi-monitor hotplug, simultaneous access
to every real device, WAN clipboard latency, measured CPU savings or full
platform parity is claimed. Rich text, images, files, Wayland clipboard and
macOS serving remain outside this text/X11 increment.

Contracts and design sources: [clipboard](../clipboard.md),
[multi-monitor](../multi-monitor.md), [execution plan](../desktop-features-plan.md),
[ICCCM selections](https://www.x.org/releases/current/doc/xorg-docs/icccm/icccm.html),
[RandR](https://www.x.org/releases/current/doc/randrproto/randrproto.txt),
[NSPasteboard](https://developer.apple.com/documentation/appkit/nspasteboard).
