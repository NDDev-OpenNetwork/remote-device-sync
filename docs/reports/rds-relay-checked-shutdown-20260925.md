# Checked owned relay shutdown — 2026-09-25

Scope: W2.5 ownership and the W3.2 runtime prerequisite, base `661a6e7`.
Linux loopback fixtures only; no live service was changed.

The previous owned relay `close()` and `drain()` returned unit and only logged
an abnormal runner exit. Its connection JoinSet also lived solely in that runner,
so a failed runner lost the ability to join those children explicitly.

Both methods now return `Result<(), ShutdownError>`, retaining the original
runner JoinError. The Relay owner shares the connection set with the runner;
normal completion and failure fallback both join it. The outcome is stored
before fallback awaits, and close waiters serialize through the retained runner
mutex. Canceling a fallback waiter preserves the outcome and child handles for
the next caller. Cleanup still uses the existing five-second connection grace,
then aborts and joins remaining async workers. Admission and wire behavior are
unchanged. This is a deliberate library API change; callers must inspect results.
See the [contract](../relay-control.md#server-task-ownership).

The new real relay/tunnel fixture aborts the accept runner through private test
access, cancels a close waiter before acquiring the shared child set and another
while joining it, then releases a controlled child. Concurrent and repeated
close calls return the same retained failure. The child set is empty, its weak
owner is released, occupied admission/history and path-driver counts are zero,
and the attached tunnel observes unavailability. Production code has no test
fault flag. Existing normal drain, forwarding, failure isolation and canceled
close fixtures now also require successful shutdown results.

Focused relay library: **10 tests passed**. Final formatting and
normal/X11/all-feature workspace Clippy passed. Workspace: **400 tests
across 70 targets**, 1 existing ignored test. All-feature
net/relay/agent/CLI: **195 tests across 39 targets**.
After the full matrix, only the fault test synchronization was tightened to
observe the actual child-set lock before cancellation. That final fixture passed
all **10 relay library tests** and all-target relay Clippy with the owned and
metrics features; production code was unchanged. Commands, durations and source
hashes are in the
[machine-readable receipt](rds-relay-checked-shutdown-20260925-data.json).
No dependency, unsafe code or external runtime helper was added.
`cargo-deny` remains unavailable locally.

Normal runner cleanup proceeds independently of canceled callers. A canceled
failure-fallback join needs another close caller to resume waiting; Drop cannot
synchronously join and needs continued executor progress. Native macOS,
process-wide panic, full deployment/network qualification and owned relay
binary integration remain open. No remediation wave is closed.
