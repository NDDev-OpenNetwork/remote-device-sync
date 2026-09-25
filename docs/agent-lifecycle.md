# Agent admission and task lifetime

The agent owns application connection tasks in `run`, and each connection owns
its bidirectional service tasks. Both groups reap completed tasks. Dropping the
runner aborts its connection tasks; their destruction aborts the nested service
tasks and closes QUIC through the connection lifetime guard. Cancellation does
not rely on dropping the last endpoint handle.

On normal transport closure a connection closes authorization state, releases
its replay reservation, aborts and joins its grant watchdog, then aborts and
joins service tasks. Metrics sampling is inline in the connection task, so no
separate sampler can retain a closed connection. `Connection::wait_closed()` is
a transport notification only; it does not promise application task completion.

Endpoint shutdown allows five seconds for connection tasks to finish normal
cleanup, then aborts and joins remaining connection tasks. Cancellation and this
fallback request nested task cancellation through destructors; they cannot
synchronously join grandchildren from `Drop`. Callers needing the normal join
path should close the endpoint and await `Agent::run`.

## Budgets

| Limit | Default | Application boundary |
|---|---:|---|
| Connections | 32 | Pending incoming handshakes plus admitted connections, shared across `run` and direct `serve` calls on one Agent |
| Service streams | 64 | Concurrent bidirectional service tasks per connection, including hello and authorization waits |
| Incoming handshake | 15 seconds | One admitted incoming handshake |
| Stream hello | 15 seconds | Reading the first framed request on an accepted stream |
| Normal shutdown | 5 seconds | Connection-task cleanup after endpoint acceptance ends |

`AgentLimits::new` takes positive 16-bit values and `Agent::with_limits` consumes
the agent before running it. The executable accepts `--max-connections` and
`--max-streams` (1–65535), validated before identity creation or bind. These are
agent options, not additional fields in the versioned endpoint JSON schema.

When connection slots are full, the runner refuses an incoming handshake
without creating an application task. Direct `serve` closes the supplied
connection and returns an error. At stream capacity it stops accepting more
bidirectional streams until a worker finishes; QUIC flow control/backpressure
remains responsible for transport buffers and queued streams. Closure is still
observed while the group is full. Permits and service counters release on every
future exit, including cancellation.

`Agent::active_connections()` counts admission slots, including handshakes;
`active_streams()` counts bidirectional service workers across the agent. The
transport metrics sampler counts established, allowed connections separately.
These gauges are diagnostics, not process-wide RSS/FD or total worker counts.

## Qualification and limits

Real loopback tests on iroh and owned noq cover runner cancellation with retained
endpoint handles, TCP forwarding socket closure, stream saturation and release,
connection refusal followed by successful readmission, direct-serve cancellation,
and normal shutdown with a blocked partial hello. Authorization tests check that
normal teardown joins the watchdog and releases the replay reservation. Binary
preflight tests reject zero, negative, overflowing and nonnumeric limits.

This does not bound all transport handshakes/buffers, uni inboxes, desktop/audio
workers, relay queues or blocking disk jobs. A service can still occupy a slot
until its own deadline/progress policy completes. Negotiated session IDs,
per-service fairness, full timeout/retry classes, global load/RSS/FD acceptance,
physical network failover and native macOS qualification remain separate work.
See the [receipt](reports/rds-agent-lifecycle-20260925.md). No remediation wave is
closed by this increment.
