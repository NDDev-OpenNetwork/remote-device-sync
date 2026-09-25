# Relay datagram work admission — 2026-09-25

Scope: W2.5/W3.2, base `991f236`; Linux synthetic loopback fixtures.

The old forwarding loop validated frame/key before its source byte bucket.
Malformed input skipped the bucket, and zero-byte input would remain free even
if that byte-only check moved earlier. A real QUIC regression failed before the
fix: empty, short and invalid-curve-key frames all left accounting untouched.
The fixture joined both endpoints and verified zero occupied admission before
asserting the three failures.

Every raw datagram is now charged first at `max(length, 1024)` units. Existing
numeric limits are 64 Mi units/s and a 4 Mi-unit burst per attachment: 65,536
small frames/s, 4,096 per full burst. Larger frames pay their actual size. Rate
exhaustion skips validation/routing and drops the packet without another queue.
Cooperative yielding every 64 datagrams remains unchanged. See the
[admission contract](../relay-control.md#datagram-work-admission).

Raw destination bytes now look up the exact key in the already authenticated
attachment table. Unknown/invalid keys cannot match, and valid forwarding no
longer decompresses a curve point for each packet. Source-slot ownership and
recent-peer lock ordering remain intact. This removes repeated work by source
inspection; no measured latency or throughput improvement is claimed.

**39 focused relay tests passed**, including the formerly failing
network case and real-binary restart/forwarding. Deterministic tests cover empty
and small-frame burst exhaustion, larger-frame byte cost, refill and idle burst
cap; they replace the old sleep-based refill test. Formatting and three Clippy
lanes passed. Workspace: **413 tests across 72 targets**,
1 existing ignored test. All-feature net/relay/agent/CLI/server:
**219 tests across 43 targets**. See
[commands, durations and hashes](rds-relay-accounting-20260925-data.json).

Small valid frames deliberately consume more allowance than before. These are
admitted application work limits, not ingress/decryption or global CPU limits;
reattachment starts a new budget. Global fairness, reconnect/control-message
limits, performance/native macOS and physical flood qualification remain open.
No wire version, dependency or external runtime helper changed. `cargo-deny`
is unavailable locally. No remediation wave closes.
