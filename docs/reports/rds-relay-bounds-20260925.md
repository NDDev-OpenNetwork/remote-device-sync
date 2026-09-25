# Relay peer ownership and receive bounds — 2026-09-25

Scope: W2.1/W2.5 and W3.6 client relay resources, base `e119fd6`.
Linux real loopback transport; no deployment or wave closure.

## Correction

Each attached relay client now owns a bounded peer registry and datagram queue.
Active peer leases prevent identity replacement and eviction; last-lease drop
leaves bounded inactivity grace. Capacity pressure evicts only unpinned entries.
Actual 32-bit synthetic alias collisions are refused with a typed error. Leases
belong to tracked weak policy tasks so streams can outlive Connection facades.
Direct connectivity survives unavailable relay registration; relay-only outgoing
admission fails promptly. A full receive queue drops new packets, and small
caller buffers drop entire packets instead of returning truncated prefixes.
Weak diagnostic handles expose occupancy and losses without retaining payloads.

Defaults are 1024 peer entries, 128 queued datagrams and 30 seconds of unpinned
inactivity grace. Strict positive optional JSON limits lower into the same runtime
configuration; route-only overrides preserve them. Old files remain valid; old
strict binaries reject explicit new fields. The public Rust registration API
now returns a lease that callers must retain. See [relay contract](../relay-control.md)
and [configuration](../endpoint-configuration.md).

## Evidence

A real failing-before test used two deterministic public identities with the
same synthetic alias: registering the second silently redirected a raw transport
payload intended for the first. After correction the second registration is
refused and only the first receives the payload. Inner QUIC identity pinning
is unchanged; this is not evidence of application plaintext exposure.

Five registry unit cases cover active capacity, eviction, inactivity, duplicate
leases, learned promotion and collision. Real tests cover a stream surviving its
facade, slot reuse after last-stream close, two full endpoint tables with direct
bidirectional traffic and immediate relay-only refusal. Queue overload proves
bounded occupancy, counted drops, no partial-packet delivery, subsequent traffic
and payload release despite a surviving diagnostic handle. Configuration tests
cover strict positive fields, defaults, lowering, incompatible modes and override
precedence. The focused run passed **52 tests across 7 targets**.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **364
tests across 66 targets** (1 previously qualified test ignored).
All-feature network/agent/CLI/relay: **161 tests across 37 targets**.
Isolated owned network/relay: **52 tests across 7 targets**.
The [machine-readable receipt](rds-relay-bounds-20260925-data.json) records
commands, durations, hashes and fixture notes. No dependency version, relay wire
format or unsafe code changed. `cargo-deny` is unavailable locally.

## Remaining work

The synthetic address namespace remains 32 bits. Grace is best effort and may
be evicted under pressure; idle expired storage is reclaimed lazily within the
cap. Noq may independently probe learned QNT addresses, so this is not a new
per-peer namespace. These bounds exclude engine buffers and global RSS, relay
server admission/task ownership, warm replacement and full-path recovery.
Physical network, native macOS and service qualification remain open.
Every remediation wave remains open.
