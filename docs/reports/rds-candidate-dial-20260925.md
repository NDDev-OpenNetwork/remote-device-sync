# Initial candidate dialing — 2026-09-25

Scope: R09/W3.1 initial race and socket-family consistency, with a local W2.6
handshake budget. Final working source is based on `92f7218`; this is Linux
synthetic evidence, not a deployment, backend promotion or closed wave.

## Failure evidence and correction

Two regressions failed before implementation: a silent first direct address
blocked a healthy second address, and a silent direct candidate blocked an
already attached relay. The old dialer awaited only its first address.

Up to eight supported direct candidates and one attached relay now race under
one 15-second absolute deadline. Each handshake pins the same endpoint identity.
Only the first authenticated success becomes an application connection. Other
attempts are canceled and joined, completed losers are closed, and no service
request or policy driver is started on them. Cancellation owns the attempt group.

The first IPv6 regression exposed two additional defects: the mux's first child
hid available IPv6 support, and an internal unsupported-family QNT probe could
terminate a healthy connection. The mux now presents consistent mapped/native
addresses, filters unsupported candidates before the cap and policy path opens,
and discards unroutable engine probes without treating another path as failed.
Real child I/O failures still need full isolation. See [contract](../candidate-dialing.md).

## Validation

Focused Clippy and **14 tests across three targets** passed: six candidate cases,
two dial-lifecycle cases and six owned-relay cases. They cover actual datagrams,
wrong identities, bounded all-silent failure, cancellation with an endpoint kept
alive, eight attempts from twelve candidates, two live addresses leaving one
winner, IPv4/IPv6 in both socket orders and connection directions, filtering
before the candidate cap, and attached relay routing in a dual-stack mux.

An initial cancellation fixture incorrectly allowed only two seconds for QUIC
engine draining. It now allows five seconds because the protocol's three initial
PTOs retain closing state for about three seconds; this is distinct from task
cancellation and remains below the handshake deadline. Initial failures and
intermediate IPv6 failures remain in private evidence. Relay teardown in focused
runs also emitted outer-connection I/O errors; those are not evidence that
active direct sessions survive relay failure, which remains unqualified.

Final formatting and default/X11/all-feature Clippy passed. Workspace:
**337 tests across 60 targets**, with one previously qualified
migration-capacity test ignored. All-feature network/agent/CLI/relay:
**128 tests across 31 targets**. Feature-isolated owned network:
**22 tests across 2 targets**. Commands, durations and source
hashes are in the [machine-readable receipt](rds-candidate-dial-20260925-data.json).
No dependency, wire version, unsafe code or external runtime helper was added.
`cargo-deny` is not installed; native macOS remains unqualified.

The existing `rds-bench` direct-loopback handshake smoke run completed **20/20**
at p50 **22.16 ms**, p95 **26.96 ms**, p99
**28.32 ms** in a debug build. [Raw report](rds-candidate-handshake-20260925.json).
It measures ordinary local handshakes, not failed-candidate latency, WAN behavior
or owned/iroh parity. The harness reuses endpoints and immediately closes after
client handshake completion: its snapshot has 20 client completions and 19 server
acceptances. It does not wait for application readiness or prove 20 service
roundtrips; measurement reconciliation stays W0 work. No minimum-latency product
claim follows from it.

## Remaining scope

R09's first-address reproduction is corrected; W3.1 remains partial for independent
relay bootstrap, address scope and interface discovery, NATs and blocked UDP.
W3.6 still needs validated path selection and full transport-failure isolation.
Global dial budgets, warm relay replacement, protocol capability negotiation,
requested ALPN narrowing, real SSH/desktop/sync recovery and native platform
acceptance remain open. No remediation wave or production readiness is closed.
