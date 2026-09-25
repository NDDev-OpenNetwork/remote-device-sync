# Exact requested ALPN — 2026-09-25

Scope: W2.2 protocol selection in owned TLS, source base `7fbe947`.
Linux synthetic evidence; no deployment, backend promotion or wave closure.

## Defect and implementation

Three real-QUIC regressions failed before the correction. A client requesting the
second configured protocol negotiated the server's first preference; a server
missing the requested protocol silently negotiated another; concurrent requests
for two protocols both received the first. The owned connect method checked that
the requested ALPN was configured but then offered the complete configured list.

Binding now creates immutable exact-ALPN client configurations, shared by endpoint
clones. Each dial and all its candidate attempts use the selected configuration
via `connect_with`; there is no mutable endpoint-default update. Locally unknown
ALPNs fail before dialing, and remotely unsupported requests fail in TLS before
service use. The expected endpoint identity remains pinned on every attempt.

One shared 256-session TLS cache preserves the original endpoint-wide bound.
Configurations share their certificate resolver and verifier as required by the
locked rustls API; each handshake still verifies its selected ALPN. No early
application data path was added, and this report does not claim successful TLS
resumption was measured. See [contract](../protocol-negotiation.md).

## Checks

Final focused Clippy and **16 tests across three targets** passed: four ALPN
cases, six candidate cases and six backend/interop cases. Repeated alternating
requests on the same endpoints exchange actual datagrams; unsupported protocols
fail; concurrent protocol requests stay independent. Port zero forces an
immediate first-candidate error before a valid candidate wins with the exact ALPN.

A trial requiring the wrong server to observe client TLS rejection within two
seconds timed out. That peer event is not a client-failure completion signal.
The existing wrong-identity regression is retained unchanged, while synchronous
invalid-address refusal supplies deterministic failure ordering. The trial log
is retained privately; it is not counted as a passing run or a production fix.

The final matrix passed formatting, default/X11/all-feature Clippy, **341
workspace tests across 61 targets**, **132 all-feature
network/agent/CLI/relay tests across 32 targets**, and **26
isolated owned-network tests across 3 targets**. One previously
qualified migration-capacity test remains ignored in the workspace.
[Machine-readable evidence](rds-alpn-selection-20260925-data.json) records exact
commands, durations and source hashes. No dependency, wire tag/version, unsafe
code or runtime helper changed. `cargo-deny` remains unavailable.

## Remaining scope

Full service capability/limit negotiation, protocol compatibility policy,
session/transfer identifiers, native macOS acceptance and real topology/service
recovery remain open. Exact ALPN selection does not close W2.2 or any remediation
wave. Validated path selection and independent transport failure handling are
subsequent connectivity work.
