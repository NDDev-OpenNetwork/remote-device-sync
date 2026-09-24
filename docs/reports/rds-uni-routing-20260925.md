# Uni-stream ownership and bounded routing — 2026-09-25

Scope: W2.5 uni-router lifecycle, base `948d66d`. Synthetic Linux loopback
fixtures. No backend promotion, deployment or remediation wave closure.

## Confirmed defects and implementation

Two new regressions failed before the fix, one on iroh and one on owned noq.
After dropping the facade Connection and its UniStreams inbox, the peer remained
connected: the router task owned a Connection which in turn owned the router's
task handle. Without explicit close, that ownership cycle could persist.

The router is now an owned private module. Connection handles and inboxes own it;
the driver holds only a backend connection and a weak owner reference. Route
workers do not retain a strong owner across an await. Dropping the last owner
aborts the driver and its task group. A retained inbox remains a valid I/O owner
and can continue receiving after the facade handles drop.

A JoinSet bounds pending tag-read/queue-handoff work to 64 tasks per connection.
Completed tasks are reaped, and closure remains selectable during saturation.
On connection death, workers are canceled and joined before producers are
cleared. Existing inboxes retain at most 128 buffered streams each; sync keeps
backpressure. The contract and limits are in [uni-routing.md](../uni-routing.md).
No wire type, protocol version, dependency or external runtime program changed.

## Tests

New real-QUIC cases cover last-owner teardown on each backend, inbox ownership
after facade clones drop, 80 partially written tags against the 64-task bound,
ready routing behind a genuinely stalled tag, kind reclaim and saturated inbox
closure. The full-inbox fixture raises only its QUIC receive credit to 512 and
fills 128 inbox slots plus 64 pending handoffs, with additional streams queued.
Close must release the workers before the retained inbox is drained.

Existing demux tests now retain the peer's Connection for the intended scenario.
Their stalled-tag case writes a partial length prefix: opening a uni stream
alone had not announced it on the wire. An initial new gauge assertion also
ran before the worker's final destructor; it was corrected to await that actual
cleanup with a bounded observation window, without relaxing the final count.

Formatting and default/X11/all-feature Clippy passed with warnings denied.
The final workspace passed **307 tests across 55 targets**, with one
previously qualified migration-capacity test ignored. The all-feature
network/agent/CLI/relay run passed **92 tests across 26 targets**;
feature-isolated uni routing passed **7 tests**.
[Machine-readable evidence](rds-uni-routing-20260925-data.json) records commands,
counts, durations and source hashes.
Full logs stay in private local evidence storage. `cargo-deny` is unavailable.

## Remaining qualification

This is not full session ownership or global resource acceptance. Session IDs,
cross-kind fairness under saturation, negotiated/configurable limits, media
queue policy, agent/service admission, relay pumps and disk-worker cancellation
remain open. Counts do not establish process RSS/FD bounds or latency percentiles.
Native macOS, real NAT/failover and deployed desktop/SSH were not qualified.
