# Mux child failure isolation — 2026-09-25

Scope: W3.6 local transport isolation and W3.7 prerequisites. Source base:
`470e0e3`. Exact source hashes, toolchain, commands and counts are in the
[machine receipt](rds-mux-isolation-20260925-data.json). Linux x86_64 only.

## Reproduced failures and changes

Both real QUIC fixtures failed before the correction: a terminal child send
or receive error stopped shared transport progress despite a validated healthy
sibling. A further regression reproduced selection of a failed last path when
the engine refused its close and another unrelated local socket stayed alive.

The mux now shares monotonic child health across senders, receiver and policy.
Each waiter has an independent failure notification; diagnostic handles retain
no I/O. Packet-scoped errors preserve the socket, retries are bounded, terminal
errors retire only their child, and all-child failure is explicit. Policy
withdraws affected advertisements, excludes failed routes, and closes held
connections when every local transport is gone. Source routing and logical
socket identity stay stable. See the [contract](../socket-failure-isolation.md).

## Validation

All seven final checks passed: workspace/shared formatting, default/X11/all-
feature workspace Clippy with `-D warnings`, workspace tests and expanded
all-feature net/relay/agent/CLI/server tests.

- Workspace: **427 passed**, 1 ignored, 73 targets.
- Expanded: **234 passed**, 0 ignored, 44 targets.
- Repeated focused campaign: **30 runs × 3 real transport tests**, no failures.
- Five poll-level tests cover wake ownership, retry/pressure, routing and
  metadata lifetime; the real fixtures require data through an existing stream,
  datagram echo, QNT withdrawal, new connection acceptance on the surviving
  socket, and last-child shutdown with handles retained.

An earlier matrix exposed a fixture assumption: it required one specific
replacement PathId to become Available, while QNT could legitimately create
and select another healthy path. Diagnostic path snapshots established this.
The corrected test waits for observation of its sibling and a healthy selected
route, then checks actual data continuity. Deadlines were not increased. The
final matrix was rerun completely; earlier failed attempts are not passes.

Cargo-deny is unavailable. Native macOS, real NIC/suspend/NAT failures,
impairment, soak and latency/resource comparisons were not run. No benchmark
speedup, deployment readiness or wave closure is claimed. Full path inventory,
socket recreation, same-IP multi-bind port routing, relay bootstrap and warm
replacement remain open. No dependency, runtime helper or wire version changed.
