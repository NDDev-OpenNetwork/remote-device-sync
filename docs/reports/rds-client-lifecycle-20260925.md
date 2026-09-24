# Client request deadlines and forwarding ownership — 2026-09-25

Scope: W2.5 local forwarding and W2.6 client request preludes, base `bf42c2d`.
Synthetic Linux loopback evidence; no deployment or remediation wave closure.

## Confirmed defects and changes

Before the fix, two real-QUIC regressions exceeded the outer 17-second test
budget: a peer supplied no bidirectional stream credit, or advertised zero
receive window while the sender's buffer was limited to one byte. All four
service operations could stall before their ACK-read timer even began.
A test fixture initially used an unavailable raw-iroh method; it was corrected
to the locked API before the two runtime failures were recorded.

One 15-second request deadline now covers opening, writing, ACK and fixed reply
completion for Authz/Ping/Info/TCP/Sync. Ping's echo shares the same budget.
Pending request streams reset/stop on error or cancellation; incomplete Authz
closes the connection. One-shot replies must end at FIN without trailing bytes,
matching the current agent. Successful TCP/Sync transfer stream ownership to the
caller, with separate body policy. No request is automatically retried.

Local TCP forwarding owns a bounded task group (default 64, positive 16-bit
CLI/library override). Saturation pauses acceptance; normal connection close
joins workers. Cancellation closes local TCP and resets remote I/O while leaving
the shared connection available. Port-zero output reports the actual binding.
See the [contract](../client-lifecycle.md) for precise guarantees and limits.

## Validation

Six request tests cover zero credit, blocked writes, cancellation/reset,
interrupted Authz and the total Ping deadline. Two forwarding tests exercise
both transports, live data, saturation, refusal/readmission, listener cleanup,
and a successful Ping on the shared connection after forwarding cancellation.
Binary tests reject invalid worker budgets before identity creation.

Formatting and default/X11/all-feature Clippy passed with warnings denied.
The final workspace passed **323 tests across 58 targets**
(one previously qualified migration-capacity test ignored). All-feature
network/agent/CLI/relay passed **109 tests across 29 targets**;
isolated CLI validation passed **13 tests across 3 targets**.
[Machine-readable evidence](rds-client-lifecycle-20260925-data.json) records
commands, durations, counts and source hashes; full logs stay in private evidence
storage. `cargo-deny` is unavailable. Existing locked iroh becomes a direct
**test-only** CLI dependency for controlled transport fixtures; no package or
version, unsafe code, wire type or runtime helper program was added.

W2.5/W2.6 remain partial: configurable/negotiated deadlines, retry policy, relay
and agent startup, desktop/media work, global resource/load acceptance and disk
cancellation remain open. Drop requests child aborts; normal paths join. No
native macOS, production SSH/desktop or real failover qualification is claimed.
