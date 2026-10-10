# Native workspace implementation — 2026-10-10

Status: implementation plan, W6/W9.2. No tabbed product acceptance claimed yet.

The current viewer can launch up to eight independent processes for explicit
device/display pairs, but has no tab bar, connection picker or in-window settings.
The requested product is one convenient workspace with multiple devices and
multiple displays, independently configurable and switchable without disconnecting
other sessions. Existing managed session identities, monitor inventory, bounded
input/clipboard queues and per-session desktop routes remain the transport owners.

1. Add a bounded workspace model with stable tab IDs, exact device/display
   identity, selection/close/reorder rules and per-tab quality/input/clipboard
   preferences. Reject duplicate conflicting intent and stale async replies.
2. Reuse the existing presentation, input translation, clipboard and reconnect
   worker for every tab. One native window/event loop/GPU owns visible rendering.
   Switching releases the old tab's held keys/buttons before routing new input;
   inactive tabs cannot publish clipboard or show another tab's texture.
3. Add a tab bar and connection/settings UI. Inspect a device through the local
   agent, list actual displays, open one or several, and isolate failures to the
   affected tab. Never create endpoint identities, issue grants or widen policy.
   Existing separate-window/headless/diagnostic modes remain explicit options.
4. Persist ordinary workspace/profile preferences with a versioned bounded
   atomic file, containing no embedded credential. Import existing viewer launch
   configuration without rewriting it. Show save errors and unsupported displays.
5. Test state ownership, stale completion, switching with held input, clipboard
   focus, viewport coordinates, reconnect/settings isolation and multi-device
   session coexistence. Verify native UI on macOS and isolated Xvfb on Linux,
   then workspace/feature checks, a registered checkpoint and an exact artifact.
6. Reconcile signed PR source, estate pin and installed runtime only after the
   relevant qualification, retaining the separate known control RTT failure.

Implementation references: the maintained [egui integration](https://github.com/emilk/egui)
and [egui-wgpu 0.36.2](https://docs.rs/crate/egui-wgpu/0.36.2) use the existing
wgpu 30 renderer family. A small optional UI dependency is preferable to owning
custom text shaping, widgets and focus management. Exact compatibility, licenses
and feature footprint must pass before adoption. References are checked against
the actual October 10 implementation, not asserted as archived September 26 pages.

The local sync checkpoint at `89db733` passes 1,072 executions; PR150's macOS
desktop-feature RTT p95 411 ms remains above the unchanged 400 ms bound. Native
build success is not acceptance of that latency requirement or of this new UI.
