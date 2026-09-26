# Desktop sender and serving-task qualification — 2026-09-26

Scope: W6.1/W2.5/W2.6 increment; no wave or release gate is closed.
Code source: `423ecb3f8a90877eb75487dcb64477a2c3722555`.
See the [sender contract](../desktop-frame-delivery.md).

A producer event interrupted a partial `write_all`, then restarted the entire
payload. Two deterministic 64-byte-backpressure regressions reproduced 4160
received bytes for a 4096-byte frame, for both keyframe supersession and producer
closure. Retaining the same pinned write fixes both. Continuation errors remain
errors, abandoned frames RESET, and one deadline covers stream credit through
payload completion.

Canceling the serving future also left the capture source alive. A real-session
regression failed before task ownership was added. The serving future now owns
writer/pacing/capture groups; normal shutdown joins asynchronous children,
control replies are bounded, and cancellation resets the control stream. Running
native capture calls may finish before observing the closed frame queue.

## Validation

Linux x86_64, Rust 1.98.1, two build jobs, one local Cargo operation at a time:

- Workspace formatting and clippy, default and X11, passed with warnings denied.
- Default desktop suite: 19 unit tests, two lifecycle scenarios and seven
  session/media tests passed (28 total).
- X11 desktop suite on an isolated 1280×720 Xvfb display: 25 unit tests, two
  lifecycle scenarios and seven session/media tests passed (34 total), including
  actual capture/DAMAGE and OpenH264 tests. The same suite on the ambient display
  failed its existing root-repaint DAMAGE assertion. This environment-sensitive
  native-display case remains open; Xvfb does not qualify a composited desktop.
- Real iroh/noq tests observe frame RESET and source release after cancellation,
  while unrelated traffic still succeeds on the held connection. Duplex tests
  also check partial-write failures and nonblocking stale-delta abandonment.
- A clean-source serial measurement passed all seven session scenarios:
  60-second iroh loopback delivered 3600 synthetic headers, age p95 4 ms/p99 6 ms,
  process peak RSS 49052 KiB. The seeded impaired noq lane delivered 426 headers,
  age p95 94 ms/p99 107 ms and control RTT p95 160 ms. Actual datagram drops were
  observed. These are single-run header measurements, not displayed-pixel
  latency, a memory ceiling, comparative improvement or WAN acceptance.

[Machine-readable reproduction data](rds-desktop-sender-20260926-data.json)
contains exact source, toolchain, command and binary/lockfile hashes. GitHub
Linux/macOS integration checks remain independent. No installed agent or
published preview changes here.

Remaining: reference-aware queue collapsing, real H.264 continuity under
impairment, per-session desktop wire IDs, global/native memory budgets, managed
viewer/rendering, input/focus release and physical platform/network acceptance.
