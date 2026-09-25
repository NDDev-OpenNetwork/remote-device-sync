# Candidate family fairness — 2026-09-25

Scope: W3.1 initial candidate budget, base `97096e2`.
Linux loopback evidence; no backend promotion, deployment or wave closure.

## Correction

Filtering unsupported families did not prevent a supported-family imbalance:
eight silent IPv4 addresses on a dual-stack endpoint consumed every direct slot
and excluded its peer's healthy IPv6 listener. The real failing-before regression
binds all silent sockets, polls the actual IPv6 listener and checks authenticated
datagram delivery within its test deadline.

Extraction now shares one canonicalizer with the path-credit queue. Mapped IPv4
aliases consume a native IPv4 slot; native IPv6 scope IDs survive unchanged.
Supported addresses are sorted within families and alternated under the same
eight-direct-candidate cap, filling unused family slots with the other family.
Only eight potential winners per family are retained during extraction, avoiding
an unbounded duplicate allocation for a larger caller-provided address record.
The attached relay remains an independent ninth candidate; TLS pinning, the
common deadline and owned loser cancellation are unchanged.
See [contract](../candidate-dialing.md).

## Evidence

Four unit cases cover family sharing, empty-family backfill, mapped deduplication
with distinct IPv6 scope IDs, and family filtering before the cap. The failing
real regression now passes. The focused library/candidate/simulation run passed
36 tests across three targets, including wrong identity, simultaneous successful
candidates, all-silent timeout, dual-stack socket order and delayed-credit retry.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **356
tests across 62 targets** (one previously qualified migration-capacity
test ignored). All-feature network/agent/CLI/relay: **147 tests across
33 targets**. Isolated owned network: **43 tests across
5 targets**. The [machine-readable receipt](rds-candidate-families-20260925-data.json)
records commands, durations and source hashes. No dependency, wire version,
unsafe code or runtime helper was added. `cargo-deny` is unavailable locally.

## Remaining work

A fixed cap may omit a working address later within one family. Progressive
probing and route history need a separate global work/admission policy. This is
candidate selection, not a staggered Happy Eyeballs algorithm or a WAN benchmark.
Relay bootstrap and replacement, path-event recovery, complete metrics, child
transport failure isolation and native-platform/physical-network qualification
remain open. No remediation wave is closed.
