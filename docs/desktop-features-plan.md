# Desktop feature execution plan — 2026-10-09

Scope: existing Linux X11 serving and macOS native viewing. Finish bidirectional
UTF-8 text clipboard, simultaneous monitors, and simultaneous device windows.
Code, native fixtures and installed observations are separate facts. PR #139 is
a draft until these concrete acceptance conditions pass.

1. Preserve V2/V3/V4 wire tags. Negotiate reverse clipboard via additive V5;
   an additive local IPC request must select it. Old local API calls stay old.
   View-only may neither read nor write the native clipboard. Test legacy
   refusal, receipt-mode ACK matching and grant/display scope.
2. Move native clipboard ownership/read state to one bounded worker per
   session. Subscribe to XFixes selection timestamps (same owner may copy
   repeatedly); use at most 4 Hz owner polling only when XFixes is absent.
   Jobs wake the worker through a local socket, not a 100 Hz timer. Validate
   property type/format/bytes_after, UTF-8, 1 MiB and INCR termination. Cancel
   stale conversions and never retransmit a failed paste. Native fixture:
   repeated same-owner copy, empty/multibyte/large text, no echo, cancellation.
3. Keep the framed control reader owned and uncancelled. Send one reverse
   chunk at a time between input/heartbeat/receipt work; abort a partial failed
   write. Reliable event queues must not evict clipboard chunks. Keep one
   offer and one outgoing transfer per session; expire them and preserve the
   latest offer when a stale request arrives.
4. Route reverse clipboard to the main-thread viewer state machine. Request
   only while its window is focused; publish only a requested matching transfer,
   with a pasteboard change-count fence. Background devices must not overwrite
   the Mac clipboard. Reset transfer state at focus loss/reconnect/close; do not
   log payloads. Translate Cmd+C/Command cut to remote Control chords, matching
   the existing explicit Command paste behavior. Exercise real AppKit text
   publication using a temporary test value and restore the native clipboard.
5. Preserve legacy X-screen numbering and add stable monitor identifiers, with
   one shared catalog used by capture/input/capabilities. Use RandR 1.5 monitor
   names/outputs, a bounded topology subscription, root offsets and checked
   coordinate arithmetic. Reject removed/reconfigured geometry rather than
   capturing/injecting into a replacement. Fixture: two RandR monitor regions,
   known pixels and pointer offsets, two X roots, removed monitor.
6. Add native viewer launch options for all displays and multiple device
   windows using the same local agent identity. Each window owns its isolated
   session/decoder/clipboard state. Preserve a selected session only as a
   default, never as a mutable route. Test two peers and two monitor routes;
   closing one must leave input/video/heartbeat on the others working.
7. Run Linux native Xvfb fixtures plus the exact CI lanes, macOS native viewer
   tests and scoped performance observations. Repair failures; no skip-to-green.
   Update current contracts/plan/report, signed atomic commits and PR checks.
   Pin the reviewed immutable RDS source through GDS only after feature
   acceptance. Any protected Herdr/Xorg lifecycle requires explicit current
   permission; prepare installation and inspect PID/cgroup/API health first.

References checked against official documentation on 2026-10-09:
[X11 ICCCM selections](https://www.x.org/releases/current/doc/xorg-docs/icccm/icccm.html),
[XFixes](https://www.x.org/releases/current/doc/xfixesproto/fixesproto.txt),
[RandR](https://www.x.org/releases/current/doc/randrproto/randrproto.txt),
[AppKit NSPasteboard](https://developer.apple.com/documentation/appkit/nspasteboard),
[QUIC RFC 9000](https://www.rfc-editor.org/rfc/rfc9000.html).
Wayland portal and macOS serving remain their existing separate platform
workstreams; this feature must not pretend to implement those stub backends.
