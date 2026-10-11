# Clipboard, preferred local dialing and monitor Spaces — 2026-10-11

Scope: W4 consented serving and W6 native workspace. Implementation is published
on the feature branch; this report does not establish installed acceptance.

## Behavior

The prepared Wayland source requests optional Clipboard v1 before the combined
RemoteDesktop Start. A granted source owns session-filtered selection signals,
up to eight clipboard leases, UTF-8-only MIME negotiation and capped nonblocking
FD reads/writes. Unsupported/refused clipboard leaves capture/input usable and
never probes Xwayland. Owner changes discard unread selection snapshots; a
later successful read of that owner remains deliverable. Explicit local
permission refresh preserves previous state/history until new consent succeeds.
GNOME restoration retains its older clipboard grant, so requesting the interface
alone does not upgrade an earlier video/input-only token.

Saved device profiles can carry four validated local socket-address candidates
with a pinned peer key/ticket. One shared one-second dial budget precedes normal
same-key resolution on transport failure. Identity authentication and service
admission remain separate; service/grant refusal never replays admission through
fallback. Monitor tasks reserve/reuse one manager connection before network I/O.

The Spaces presentation owns one event loop/process and authoritative bounded
workspace, with one window/surface/input mailbox per monitor. Actual WindowId
routes input/closure. Controls begin collapsed with the existing restore arrow;
settings and reconnection retain that window. A shared Copy fence and one
five-second destination/revision/epoch/generation-bound Paste protect delayed
cross-window work. Event-loop exit seals paste before further callbacks.

## Completed checks

Mac source `fcec25e`: registered W6 close, 1161 passing test executions, zero
failures, two explicit qualification ignores. Those ignores require installed
OpenSSH tools/an unlocked isolated account and a 4096-identity signed migration
capacity qualification. Default/workspace and desktop-feature strict Clippy,
managed multi-monitor routing, persisted profiles, wire discriminants and shared
Copy fence pass. Real Iroh/Noq manager cases cover direct dialing with unavailable
directory, failed-hint ticket fallback, simultaneous monitor deduplication,
invalid/self-peer hints and final admission refusal.

The same source passed all-feature cargo-deny, matched Mac agent/CLI/viewer and
isolated preview release builds, and strict sealed application signing. Source
`3abc1ee` differs only in the Linux-gated portal clipboard file; the Mac binaries
remain attributed to their original qualified source. No artifact is relabelled.

Registered W6 benchmark artifacts retain 100 samples of 1024 model transitions
for one/four/eight tabs. Their recorded dirty state includes generated evidence;
these are model-only timings, not GPU, network or input-to-visible-pixel results.

Linux source `64915b5` passed strict portal agent/desktop Clippy and three FD/MIME
regressions. Source `9c56c95` completed registered W4 and an immutable portal-agent
release build. A later `fcec25e` resource-bound full qualification timed out
while compiling the agent tests, after 21 portal tests passed; it is a failed
qualification attempt, not a completed gate. The latest owner-snapshot correction
completed its separate resource-bound Linux qualification at `3abc1ee`: registered
W4, strict viewer-feature Clippy, matched agent/CLI/viewer release and fresh-consent
helper all passed with the pinned Rust 1.98.1 toolchain. Prior failed output is
retained. These artifacts are not installed acceptance.

The subsequent audit added real directory/transport coverage and reproduced a
previously untested wrong-key hint failure. Iroh retained the hint across dials:
the fallback Initial raced the wrong peer again, whose TLS refusal beat the valid
relay. The new explicit actor-acknowledged cleanup removes only the specified
Unknown/Unusable IP hints before fallback; Open/Inactive path history and other
peers/transports survive. No key or admission check changes. The new integration
test verifies dead, wrong-key and previously authenticated/reassigned hints,
one signed-directory fetch and the pinned
peer: Iroh uses a real isolated relay; Noq uses isolated direct transport because
its relays use a different owned protocol. The earlier Noq fixture attempted an
Iroh URL and correctly failed configuration validation; that fixture was corrected.
The initial pre-sample metrics assertion was also corrected to await the existing
one-second manager sampler, with a bounded deadline and unchanged payload check.
The reused-address case reproduced failure after the first correction: historical
Open/Inactive state alone cannot establish current validity. Cleanup now observes
ready closures and protects only paths validated by live authenticated connections.
Both manager tests, all six directory cases, the live-path preservation unit and
strict changed-crate desktop/Noq Clippy pass. Standalone vendor testing uses the
existing owned Noq patches explicitly; its generated lock changes were restored.

The first subsequent Mac W6 run failed in the direct metrics fixture after a
30-second dial timeout. The hint-cleanup operation is not used by that fixture.
Diagnostic observation showed that the fixture advertised ambient LAN/VPN
interfaces; a later diagnostic run passed, which does not explain or erase the
failure. The fixture now binds loopback explicitly, preserving its real UDP
payload/counter assertions while removing network-roaming dependence. Both
direct and relay metrics cases pass after this fixture correction. A new full
checkpoint is required; the failed attempt is retained.

The published `888cba0` source CI has 17 successful checks and two policy skips
(workflow-dispatch MSRV and branch release publication). This is separate from
the earlier retained latency failure below. The cache correction changes Mac
and Linux production inputs: earlier `fcec25e`/`3abc1ee` artifacts and their source
equivalence cannot be used as qualification for this correction. Fresh checks,
artifacts and native acceptance are required before deploying it.

## Open acceptance

Native window creation/order, gesture navigation, independent closure,
collapsed-panel restoration and installed Unicode/multiline/repeated Copy and
Paste remain unobserved for this increment. macOS owns Space placement; no private
Space-number APIs are used. Native consent and source-inventory revalidation must
precede the coordinated installed update. Rich MIME, image/file clipboard and
pre-login Wayland access are not provided by UTF-8 text support.

The unchanged impairment workload also retains an independent control-tail
failure: [macOS job](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38135852483/job/114459512183)
completed 601 probes with RTT p50/p95/p99 164/431/555 ms against the 400 ms p95
budget. Frame p95 was 204 ms and queue p95 1 ms; other 13 session-v2 tests passed.
This is not established as a regression from this increment. The prior
[investigation](rds-control-startup-pacing-20261011.md) remains open. No threshold,
seed, workload or successful retry replaces the failed observation.

Primary contracts: [Clipboard portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Clipboard.html),
[GNOME restore implementation](https://github.com/GNOME/xdg-desktop-portal-gnome/blob/50.0/src/remotedesktop.c),
[winit fullscreen](https://docs.rs/winit/0.30.13/winit/window/enum.Fullscreen.html),
[Apple Spaces](https://support.apple.com/guide/mac-help/work-in-multiple-spaces-mh14112/mac),
and [Tokio watch receiver](https://docs.rs/tokio/latest/tokio/sync/watch/struct.Receiver.html).
