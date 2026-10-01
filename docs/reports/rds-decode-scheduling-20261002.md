# Decode scheduling and cancellation — 2026-10-02

Passive native-client records grouped repeated five-second decode failures
during overlapping workstation builds. This is correlation, not proof of
a codec defect or resource cause. The earlier error did not distinguish
global decoder admission, blocking-pool queue delay and native work.

A bounded work probe now records those phases and local elapsed durations
on slow completion or cancellation. Both managed and direct decode retain
existing deadlines, decoder count and native-work permit ownership. Metadata
contains no screen, key, clipboard or peer contents.

Cancelled callers also request abort of not-yet-started blocking work. A
controlled single-thread blocking-pool fixture keeps another task occupied,
queues a decode and cancels its caller before releasing the pool. Without
the abort request, the abandoned work still runs; the regression rejects
that result. A separate already-running fixture proves cancellation keeps
the global permit until the native call returns. These follow the documented
[Tokio blocking-task cancellation boundary](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html).

Software and installed-device qualification are recorded as they finish.
Current native user latency/quality and sustained contention gates remain
open; no milestone checkpoint is closed. Local qualification builds use
limited jobs and lower process priority so diagnostic work does not consume
all interactive workstation capacity.
