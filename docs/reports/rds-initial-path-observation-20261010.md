# Initial established-path observation — 2026-10-10

The macOS standby failure retained after PR146 reproduced in PR147 at
`2eff356`: the client reported paths 0 (custom) and 1 (UDP), while the server
reported only path 0 despite both endpoints publishing their loopback sockets.
This distinguishes an observation failure from a missing configured address.

The Iroh actor subscribed to Noq path events during connection registration,
then seeded only path 0. Noq's bounded broadcast stream does not replay older
events. The QUIC handshake and additional-path establishment can complete while
the application's accepting future waits to be polled, so the server could
permanently miss a path established before its actor subscription.

A deterministic regression accepts the underlying handshake but delays polling
the Iroh accepting future until the client observes both transports. Before the
fix, the server retained only path 0 and failed the two-second observation bound.
With the fix, the same test passed in 0.08 seconds. The existing standby test's
five-second limit is unchanged.

The actor now subscribes first, reads a protocol-owned snapshot of initially
established non-abandoned IDs, and registers each. Identical queued events are
idempotent, including metrics and open notifications. The original handshake-path
relay restoration policy remains. No path-ID range is inferred from the number
of concurrent paths, and no address is invented or advertised by the observer.

The [Noq adapter](../../vendor/noq/RDS-PATCH.md) was imported byte-for-byte from
the checksum-verified published 1.3.0 archive before adding the snapshot forwarder.
The existing [protocol fork](../../vendor/noq-proto/RDS-PATCH.md) owns the actual
snapshot predicate. Tests exclude pending initial validation, abandoned retained
state and closed connections, and cover IDs beyond the concurrent-path limit.
Later address migration/validation still follows the engine's existing rules.

This follows the documented [Noq event stream](https://docs.rs/noq/1.3.0/noq/struct.Connection.html#method.path_events)
and its bounded Tokio broadcast implementation: subscribe before taking a
snapshot, then tolerate overlapping observations. A stream alone is not a
complete initial-state snapshot. The change adds no wire frame, cryptographic
operation, unsafe block or new dependency version.

At clean source `971de4b`, the registered `w3-path-snapshot` checkpoint passed
formatting, strict workspace Clippy, workspace tests, all 113 network library
tests and all 430 protocol-engine tests: 1,469 successful test executions,
zero failures and two explicitly ignored checks. Native cargo-deny 0.20.2 also
passed all four policy categories. The first checkpoint stopped on Clippy's
async-yields-async lint: returning the unpolled accepting future is intentional
in this regression. A narrowly scoped documented test annotation fixes that
lint without changing production behavior or delaying the test further.

The [handshake measurement](bench-w3-path-snapshot.md) and
[raw JSON](bench-w3-path-snapshot.json) record 100/100 successful local Iroh
handshakes: p50 0.90 ms, p95 1.16 ms, p99 1.82 ms. This is a smoke cohort,
not a before/after improvement or physical/WAN latency result. GitHub results
are separate exact-head evidence.

Later event-stream overflow reconciliation, complete
retired-path accounting, physical topology parity and installed qualification
remain open; this report closes none of those by implication.
