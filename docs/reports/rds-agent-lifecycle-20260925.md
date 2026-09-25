# Agent task ownership and admission — 2026-09-25

Scope: W2.5 agent connection/service lifetime and initial W2.6 incoming handshake
and shutdown deadlines, base `1e046e2`. Synthetic Linux loopback evidence.
No deployment, backend promotion or remediation wave closure.

Two new real-QUIC regressions failed before the fix: aborting the agent runner
left its child session alive, on both iroh and owned noq. The endpoint and Agent
were deliberately retained. Connection and service tasks now live in owned
JoinSets, with completion reaping, cancellation and normal teardown joins.
Metrics sampling is inline; normal authorization cleanup also joins its watchdog
and releases the grant reservation.

A shared semaphore bounds pending incoming handshakes plus admitted connections
(default 32) across `run` and direct `serve`. Each connection bounds bidirectional
service tasks (default 64). Positive 16-bit flags validate before identity or
network side effects. A full agent refuses new connections; full service groups
apply backpressure. Incoming handshakes have a 15-second deadline; normal runner
shutdown has a five-second budget before parent task abort/join fallback.
See [contract](../agent-lifecycle.md) for exact ownership and cancellation limits.

## Tests

Six lifecycle tests cover both transports, retained handles, partial stream
hellos, admission refusal/recovery, direct serve, and normal close. A real TCP
socket receives a byte through the agent then observes EOF after runner
cancellation. A unit test observes the grant watchdog finished and replay slot
released after normal teardown. Actual binary tests reject invalid limits before
creating an identity. Focused validation passed 19 tests across three targets.

Formatting and default/X11/all-feature Clippy passed with warnings denied.
The final workspace passed **314 tests across 56 targets**
with one previously qualified migration-capacity test ignored. All-feature
network/agent/CLI/relay validation passed **100 tests across 27 targets**.
[Machine-readable evidence](rds-agent-lifecycle-20260925-data.json) records
commands, source hashes, durations and counts. Full baseline/final logs remain
in private local evidence storage. `cargo-deny` is not installed.

No dependency, wire version, unsafe code or runtime helper changed. Limits are
application task/slot counts; they do not establish global RSS/FD, media/disk/relay
worker ownership or per-service fairness. Drop requests nested cancellation;
normal teardown joins, but Drop itself cannot synchronously join grandchildren.
The 15-second stalled-handshake and five-second fallback thresholds were not
qualified by a separate long-stall campaign. Native macOS, real NAT/failover and
deployed desktop/SSH acceptance remain open.
