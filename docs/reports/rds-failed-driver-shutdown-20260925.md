# Shutdown after a stopped QUIC I/O driver — 2026-09-25

Scope: W2.5/W2.6 explicit owned-endpoint shutdown, base `b503a2e`.
Linux real UDP with injected sender failure; no deployment or wave closure.

## Correction

Noq endpoint close queues events to per-connection protocol drivers. A fatal
sender I/O error can already have stopped one of those drivers. With multiple
I/O handles still alive, the queued close is never applied and the weak policy
task can wait indefinitely for a connection-closed notification.

The endpoint's serialized admission boundary now closes its task tracker and
cancels an owned shutdown token. Each tracked policy future observes that token,
upgrades its weak connection handle only long enough to close state directly,
and exits. Endpoint close still waits for owned policy tasks. Ordinary connection
closure and last-I/O-drop behavior remain weak; subscriptions are created before
spawning, and no strong I/O handle is held across an await. The change uses the
existing tokio-util dependency without adding tasks or runtime helpers.
See [contract](../path-selection.md).

## Evidence

The regression first transfers an authenticated datagram, enables an injected
BrokenPipe and confirms the sender returned the error. It retains Connection and
Path handles while requesting endpoint shutdown. Before the correction, close
exceeded a one-second deadline; direct connection close was used only for fixture
cleanup. Afterward endpoint close returns, the retained connection reports closure
and the endpoint owns zero policy tasks. The initial single-handle fixture passed
through implicit close and was insufficient to reproduce this failure.

The focused driver/lifecycle/uni-routing/simulation run passed 15 tests across
four targets, including last-handle drop, streams outliving facades, connection
churn, concurrent endpoint close, pending candidate retry cancellation and path
validation behavior.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **357
tests across 64 targets** (one previously qualified migration-capacity
test ignored). All-feature network/agent/CLI/relay: **150 tests across
35 targets**. Isolated owned network: **40 tests across
5 targets**. The [machine-readable receipt](rds-failed-driver-shutdown-20260925-data.json)
records commands, durations and source hashes. No dependency version, wire format
or unsafe code changed. `cargo-deny` is unavailable locally.

## Remaining work

This closes admitted local connection state; it neither restarts a failed QUIC
driver nor proves generic socket failure isolation, remote close delivery or
complete draining of underlying packet/history state. In-flight handshakes and
application/relay service groups retain their own lifetimes. Global resource
bounds, relay queue/peer-map ownership, real interface/NAT transitions and native
macOS/service qualification remain open. Every remediation wave stays open.
