# Relay server lifecycle and failed-path retirement — 2026-09-25

Scope: W2.5/W2.6 and W3.1/W3.3/W3.6, base `fb1fc65`.
Real Linux loopback transport; no deployment or wave closure.

## Correction

The owned server reserves a positive connection budget before spawning a
handshake. Permits cover registration, service and detach cleanup; excess
attempts are refused. The runner owns a JoinSet, reaps completions, closes the
endpoint and gives children five seconds to finish before aborting/joining.
The default budget is 256 connections; a separate 15-second handshake deadline
precedes the existing registration deadline. The new limits are library API,
not yet production CLI integration.

Relay Drop seals registration, closes current tunnels and requests runner
cleanup. Explicit close joins it; stored ownership survives a canceled caller.
The runner also owns two-second drain grace, so canceled or concurrent callers
cannot leave a half-drained server. Registration guards unlink only the current
owner and its recent-flow references on cancellation. Replaced sources cannot
reuse successor state. At most 16 notice futures are polled inline per broadcast,
without separately spawned writer tasks. Forwarding yields every 64 frames.

A coupled path-selection gap was exposed by the shutdown checks: old low RTT
could leave a failed relay preferred over working direct connectivity. A shared
tunnel availability watch now wakes the existing connection policy, filters
queued synthetic candidates, withdraws local synthetic QNT advertisement and
excludes known failed relay paths. Closable paths are abandoned; a failed last
open path stays Backup and weakly tracked until closure becomes possible or
transport timeout applies. Fresh endpoint addresses and dials omit failed relay
routes, and synthetic local IPs are never published as direct candidates.
See [relay contract](../relay-control.md) and [path policy](../path-selection.md).

## Evidence

Two failing-before tests observed a live tunnel after Relay Drop and a server
still running after cancellation of a started drain. Six new server cases cover
those failures, concurrent drains, canceled/repeated close, silent registration
admission/refusal/reuse, and cancellation after a real forwarded frame created
flow history. Existing framed notices, usable grace traffic and replacement
ownership remain tested.

The first server-only correction exposed an intermittent existing policy gap.
Diagnostics reproduced the first request arriving but its reply using the dead
relay; both inner connections were still open. The direct path was Backup while
the relay retained Available with lower historical RTT. A diagnostic campaign
stopped at its seventh run after reproducing that state; the deadline was not
raised to hide it.

The managed regression now proves actual STREAM frames over the relay before
failure, retirement/direct selection within one second, 25 subsequent datagram
roundtrips, no fresh failed-relay advertisement and prompt stale-ticket refusal.
The focused lifecycle/relay/simulation run passed **73 tests across 11 targets**.
The final compiled failure-isolation target passed **30 repetitions / 60 test
executions**. These are bounded correctness checks, not WAN performance or
transition-time datagram delivery guarantees.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **364
tests across 66 targets** (1 previously qualified test ignored).
All-feature network/agent/CLI/relay: **167 tests across 37 targets**.
Isolated owned network/relay: **73 tests across 11 targets**.
The [machine-readable receipt](rds-relay-lifecycle-20260925-data.json) records
commands, durations, source hashes and repeated checks. No dependency version,
wire format or unsafe code changed. `cargo-deny` is unavailable locally.

## Remaining work

Drop needs a running executor for asynchronous cleanup. Admission limits bound
application workers/history, not all QUIC allocation or process RSS/FD usage.
Raw socket injection without RelayHandle has no tunnel health signal; unobserved
engine paths, remote-only relay failures, arbitrary direct-link failure and
complete event reconciliation remain open. This does not implement a warm
replacement tunnel, service replay or uninterrupted migration. Owned runtime
CLI integration, physical-network/native-macOS qualification and the full
SSH/desktop/sync acceptance plan remain pending. Every remediation wave stays open.
