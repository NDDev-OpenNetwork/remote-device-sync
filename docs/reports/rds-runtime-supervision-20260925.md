# Relay and directory runner supervision — 2026-09-25

Scope: W3.2/W2.5; base `336dcb6`. Linux synthetic fixtures only.

Source review found both binaries waited only for shutdown signals after
readiness. An exited relay or directory runner could leave its process alive.
They now observe runner completion and initiate joined shutdown on unexpected
exit, reporting failure even if the runner itself returned successfully. The
host retains both cleanup errors when both services fail, including startup
cleanup; signal-handler failure also retains cleanup context.

Directory and owned Relay expose cancellation-safe, repeatable observation
using their existing retained runner outcomes. Observation does not request
shutdown; close/drain still owns cleanup. Directory observation returns before
fallback storage jobs finish, so the host can first initiate its relay stop.
The iroh adapter saves its upstream supervisor result immediately after joining;
subsequent shutdown returns that result instead of polling the consumed handle
again. See [runtime](../relay-runtime.md), [directory](../directory-lifecycle.md)
and [owned relay](../relay-control.md#server-task-ownership) contracts.

**74 focused library/binary-unit tests passed**, with 1 existing
ignored test. They prove a canceled observer leaves real listeners usable,
concurrent observation/close does not deadlock, actual owned runner abort retains
the same failure allocation across observation/cleanup, and a directory panic
becomes observable while controlled disk jobs remain blocked. An actual upstream
iroh no-service supervisor error survives two observations and shutdown. The
host outcome check rejects unexpected clean completion and retains both nested
error causes. Existing real-binary traffic, restart, preflight and signal tests
are included in the full matrix. No production fault-injection flag was added.

Formatting and default/X11/all-feature Clippy passed. Workspace: **417
tests across 72 targets**, 1 existing ignored test. All-feature
net/relay/agent/CLI/server: **224 tests across 43 targets**.
[Commands, durations and source hashes](rds-runtime-supervision-20260925-data.json)
record the final results. No new dependency, wire version or runtime helper.
`cargo-deny` is unavailable locally.

This detects runner termination, not hung tasks, every worker error or remote
reachability. Close still awaits started filesystem calls; no forced kernel-I/O
deadline or automatic recovery was introduced. Iroh owns its internal task/panic
cleanup. Native macOS, deployed service and complete network/session qualification
remain open. No remediation wave closes.
