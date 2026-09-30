# Native viewer and software cadence increment — 2026-09-30

This receipt covers W6.1–W6.4/W6.6/W6.7 implementation progress and the
associated sync cancellation repairs. It does not close a remediation wave or
the hardware-codec/geometry/physical display gates. Generic measured values
are in [the companion data](rds-native-viewer-20260930-data.json); deployment
identities, screenshots, credentials and host-specific runtime receipts stay
outside this public module.

## Result

`rds desktop` and `rds session desktop` now open a native window when built
with `desktop`. The keyless `rds-viewer` entry point and a generic macOS app
packager support application launch. Winit owns OS events; wgpu owns GPU
presentation; RDS owns bounded input/frame queues, shader/viewport and pinned
session recovery. Managed commands reuse the running agent's identity.
See [the native contract](../native-viewer.md) for configuration and limits.

Completed frame ordering preserves an unfinished reference keyframe when its
delta completes first. A bounded gap triggers IDR recovery. The media reader
limit includes completed queued bodies, and canceled blocking decode work
retains its global permit. Native input preserves click/key order across
coalesced moves and releases held state on focus loss.

X11 readiness replaces fixed 25 ms idle polling. Idle wakes and sub-slot
scheduler jitter no longer count as starvation. The software producer uses
measured capture/conversion/codec work to schedule an achievable cadence under
`max_fps`; fixed CPU cost cannot collapse network bitrate simply because it
exceeds a 60 FPS interval. Sender stage timings and surface/redraw counters
make queueing and presentation progress independently inspectable.

Sync cancellation checks now precede both empty-file staging and final install.
A canceled final store cannot publish over the previous destination. Blocking
filesystem permits survive cancellation of their async waiters, and abandoned
cancel watchers are aborted. Explicit iroh loopback binds exclude implicit
other-family sockets and port mapping; regression fixtures no longer depend on
public discovery. Noq draining-path comparison uses saturating RTT arithmetic.

## Evidence and limits

The observed profile used software H.264, X11 capture at 1920×1080,
explicit 1280×720 video output, a Metal client and a roughly 86 ms control RTT.
The native window showed the actual target, and an isolated target canvas
observed mouse and physical key events. A Finder launch presented its first
frame in 551 ms on `ed4ab2a`; a subsequent 300-second `57caecc` run submitted
1213 of 1217 received frames, with receive-to-submit p50/p95 of 11.42/16.17 ms.
Those are GPU submission timings, not physical display timestamps.

Two 24-event pixel-response runs used the same full-window screenshot method.
The idle-cadence fix reduced the input-to-screenshot-completion p50/p95 from
1350.93/1563.24 ms to 456.33/496.83 ms. The capture command itself cost about
200 ms; these are upper bounds including observation overhead, not
physical glass-to-glass measurements.

On `c0a37cc`, a 7194-frame sender trace had capture/encode p50/p95 of 23/29 ms.
Its final 1024-frame window had encode-to-send p50/p95 of 0/1 ms, a 5 ms maximum
and an 8 Mbit/s target. This validates removal of the previously observed
500 ms sender queueing under the declared profile. Visible pixel response
and controlled service-fault recovery still require qualification on this
last software-cadence revision.

Local macOS default/desktop clippy and workspace/desktop tests passed. Linux
X11 workspace clippy, desktop integration tests and isolated two-screen Xvfb
capture/input tests passed. The new cadence, reference ordering, bounded input,
cancellation/publication and bind regressions passed. All-feature cargo-deny
passed after excluding winit's unnecessary Adwaita/font decoration dependency.
GitHub CI, supply-chain, CodeQL and both native package lanes passed on
`c0a37cc`; skipped MSRV/tag-publication jobs were not counted as passes.

Native codec/capture extensions, Unicode/IME, clipboard/audio, changing monitor
geometry, concurrent seat ownership, automatic issuer renewal, impaired-network
soak and physical pixel/power measurements remain separate acceptance work.
