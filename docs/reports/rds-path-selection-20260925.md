# Validated application path selection — 2026-09-25

Scope: W3.6 application eligibility and subscription ordering, base `a27f90e`.
Linux synthetic evidence; no backend promotion, deployment or wave closure.

## Correction

A real silent-path regression failed before the fix: an additional pending path
was immediately marked Available. The application also seeded pending PathIds
into its RTT selector and opened paths before subscribing to their validation
events. A path's existence/status does not prove that validation completed.

Only the authenticated handshake path is now initially eligible. Drivers subscribe
before explicit advertisements, QNT initiation or extra-path opens. Additional
direct/relay/learned paths start Backup; an Established event admits their weak
handles to the eligible set. The existing 5 ms RTT switching margin remains.
Duplicate learned candidates do not repeatedly grow pending history or attempt
counts. Arbitrary seed IDs were removed from the internal driver API.

The underlying noq scheduler already blocks application payload on unvalidated
paths. This increment corrects application selection and priority state, rather
than claiming a previous bypass of that engine protection. Task ownership still
uses weak handles and does not retain a strong connection across awaits.
See [contract](../path-selection.md).

## Evidence

Two real tests cover a silent additional path and a validated unadvertised second
listener. The latter checks its remote destination, closes the primary logical
path, observes promotion and receives the replacement datagram. A deterministic
simulation imposes 200 ms one-way delay, verifies the working RTT exceeds 350 ms,
then adds a silent candidate whose initial RTT estimate is lower. The candidate
stays Backup, the working primary stays Available, and datagram echo succeeds.
The existing partition/repair simulation also passes: four focused cases total.

An early focused run exposed a fixture assumption: spare peer connection IDs may
arrive after TLS completion, so an immediate extra open need not allocate a path.
The fixture now waits within a bounded budget for the actual pending-path
precondition. Production credit retry remains open and is not implied by that
fixture. One simulator draft also required an explicit u32 PathId literal.
Initial failed logs remain private; neither is counted as a passing run.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **344
tests across 62 targets** (one previously qualified migration-capacity
test ignored). All-feature network/agent/CLI/relay: **135 tests across
33 targets**. Isolated owned network: **25 tests across
4 targets**. The [machine-readable receipt](rds-path-selection-20260925-data.json)
records commands, durations and source hashes. No dependency, wire version,
unsafe code or runtime helper was added. `cargo-deny` is unavailable locally.

## Remaining work

Candidate retry when connection IDs/path credits are temporarily unavailable,
event-loss reconciliation, complete path enumeration and selected/relay metrics,
transport-failure isolation, physical interface/NAT transitions and service
interruption budgets remain required. The simulation's existing socket adapter
is not production RSS/FD/task evidence. Native macOS is unqualified. A logical
path-close test cannot establish physical failover or SSH/desktop recovery.
W3.6 and every remediation wave remain open.
