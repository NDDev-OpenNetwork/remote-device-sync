# Nonzero virtual relay ports — 2026-09-25

Scope: W3.1 valid identity reachability, base `33005ec`.
Linux real owned relay; no deployment or wave closure.

The deterministic address hash could produce a zero port for a valid endpoint
identity. Noq rejects that address before initiating the inner handshake. An
offline bounded Rust search found a public fixture; the real before-fix test
failed with InvalidRemoteAddress, rather than reaching its traffic deadline.

Only a raw zero hash port now maps to virtual port one. Every previously
nonzero mapping stays unchanged, the endpoint key is preserved and no relay
frame format changes. The bounded registry still refuses conflicting aliases;
this is not a collision-free namespace redesign. Older dialers retain their
zero-port calculation; mixed-version behavior for the affected class is not
qualified here. See [contract](../relay-control.md).

The regression uses only RelaySocket transports for both inner endpoints,
with no direct child that could hide the failure. It establishes pinned TLS
and reliable request/response streams in both directions, verifies peer
identities, observes real relay forwarding and performs bounded cleanup.
Normal tests use the fixed public seed and perform no search.

The focused run passed **59 tests across 8 targets**, including prior collision,
queue pressure, tunnel failure, framing/grace and task-lifecycle checks.
The initial all-feature run timed out in the existing full-inbox uni-router test.
Five isolated repetitions of the unchanged binary passed. The fixture now
identifies the backend/stage and last queue/worker counts on failure; its
deadline and production router are unchanged. The original timeout cause
remains unresolved; subsequently observed host pressure is not proof of cause.
The complete checks below were rerun after adding those diagnostics.

Formatting and default/X11/all-feature workspace Clippy passed. Workspace:
**364 tests across 67 targets** (1 previously qualified test ignored).
All-feature network/relay: **111 tests across 25 targets**.
Isolated owned network/relay: **59 tests across 8 targets**.
The [machine-readable receipt](rds-relay-zero-port-20260925-data.json) records
exact commands, durations and hashes. This small address correction uses the
network/relay all-feature suite; the separate agent/CLI all-feature suite was
last qualified at `33005ec`. No dependency or unsafe-code change.
`cargo-deny` remains unavailable locally.

This proves the formerly failing identity works through the owned loopback relay,
not physical topology coverage, mixed-version rollout or a latency target. Native
macOS, complete path recovery, owned server runtime integration and full service
qualification remain open. Every remediation wave stays open.
