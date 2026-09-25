# Relay-route failure isolation — 2026-09-25

Scope: W3.6 relay-link send boundary, base `59c0575`.
Linux real loopback transports; no deployment, backend promotion or wave closure.

## Correction

A real multipath test validated a relay path beside a working direct connection,
closed the relay and explicitly pinged the failed route. Before the correction,
direct traffic stalled and endpoint cleanup also exceeded its deadline. Noq's
connection driver returns early on a sender I/O error; propagating one relay
link's failure therefore disrupts the connection using other healthy paths.

RelaySender now treats closed tunnel, unavailable datagram support and an unknown
synthetic peer mapping as packet loss on that route, as it already did oversized
packets. Generic UDP child errors retain their behavior. QUIC can detect loss
and abandon that relay path while continuing over a direct route. The new weak
`RelayHandle::is_available()` reports local tunnel/datagram availability without
retaining I/O. Drain remains usable during grace; a destination's reachability is
not implied by local availability. See [contract](../relay-control.md).

## Evidence

After observing both tunnel handles become unavailable, the test exchanges 25
datagram requests/replies while pinging the failed relay path, then joins normal
endpoint shutdown within its budget. Another test starts with an unknown relay
mapping, checks direct traffic survives, registers the peer and waits for the
same pending relay path to validate. Retained-handle/socket-drop and Drain-grace
tests also check availability state.

The original unbounded cleanup fixture had to be stopped. A bounded rerun then
reproduced separate traffic and cleanup stalls. The missing-mapping negative
control restored its original error-return branch while the closed-link fix and
weak diagnostic remained; it failed independently. Neither negative run is
counted as successful. The focused relay suite passed 16 tests across six targets
(including zero-test binary/doc targets); the final matrix adds the grace-state
assertion to the same suite.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **356
tests across 63 targets** (one previously qualified migration-capacity
test ignored). All-feature network/agent/CLI/relay: **149 tests across
34 targets**. Isolated network/relay: **35 tests across
4 targets**. The [machine-readable receipt](rds-relay-failure-20260925-data.json)
records commands, durations and source hashes. No dependency, wire version,
unsafe code or runtime helper was added. `cargo-deny` is unavailable locally.

## Remaining work

This does not isolate generic UDP socket failures, restart a stopped protocol
driver, make every shutdown independent of that driver, or migrate relay-only
sessions to a warm replacement. Peer-table bounds/collision integrity, queue
bounds, automatic mapping lifetime, global admission and server task ownership
remain open. Physical interface/NAT changes, SSH/video/sync interruption budgets
and native macOS qualification remain unproven. Every remediation wave stays open.
