# Client request and forwarding lifetime

These shared operations now live in `rds-client`; `rds-cli` re-exports them for
compatibility. The agent-owned [local manager](local-sessions.md) layers bounded
same-UID IPC and session selection above the same request/stream guards.

Client request preludes have one 15-second deadline covering QUIC stream-credit
wait, framed request write, acknowledgement and any fixed response payload. This
applies to Authz, Ping, Info, TCP open and Sync open. Ping's echo uses the same
budget, not a fresh timeout after the acknowledgement. Dialing retains its
separate 30-second budget; authorized connect can therefore spend up to 30 seconds
on dialing and another 15 on authorization.

A pending request owns both QUIC streams. Error, timeout or cancellation resets
the request writer and stops the response reader, rather than leaving a default
writer drop to finish buffered request bytes. An ordinary service failure leaves
the shared connection available for other services. Reset is cancellation of
I/O, not a guarantee that a remote side effect already performed is rolled back.
Requests are not automatically replayed.

Authz is a connection transaction: failure or cancellation after connection
establishment closes that connection, including cancellation after a partial
reply. This lets the server's normal connection ownership release any grant
reservation. It does not promise synchronous remote cleanup before returning.
Successful one-shot responses (Authz, Ping, Info) must end after the declared
payload. The client reads FIN within the same budget and rejects trailing bytes.
The current agent already finishes these responses. Observing Authz FIN before
releasing request ownership also avoids racing client STOP_SENDING against the
server's ACK finish/commit path; service admission still rechecks committed state.

TCP and Sync successfully return their streams to the caller. Their long-lived
bodies do not inherit the request deadline; progress/idle/cancellation policy
belongs to the corresponding service owner. This change does not alter desktop
or media request handling.

## Local TCP listeners

`forward_listener` keeps its existing signature and uses a 64-worker default.
`forward_bound_listener` accepts a pre-bound Tokio listener, a validated
`TcpTarget`, and a positive 16-bit worker budget. Each accepted local TCP socket
owns a worker covering remote open and bidirectional copying. Completed workers
are reaped. At capacity the listener pauses application acceptance; the OS
listen backlog and transport buffers are separate from this limit.

`rds ssh` and `rds forward` accept `--max-connections` (1–65535, default 64).
Argument parsing rejects invalid values before identity creation or dialing.
A port-zero bind reports the actual allocated local address.

Normal QUIC closure drops the listener, cancels and joins all workers, then
returns. An accept error follows the same cleanup. Canceling the listener future
aborts its owned workers; their destruction closes TCP and resets/stops QUIC I/O.
Drop itself cannot synchronously join children. The shared QUIC connection stays
available to other callers when only the forwarding future is canceled.

## Evidence and remaining work

Real QUIC regressions with zero stream credit and zero receive window failed
before the correction. Coverage also includes partial Authz cancellation,
request resets, the shared Ping deadline, saturated forwarding admission,
readmission after refusal, live TCP splice cancellation and connection-close
cleanup. Forwarding tests exercise both iroh and owned noq. The deadline fixtures
use iroh's low-level transport configuration to force the stalled conditions.

These are application request/worker guarantees, not complete timeout policy or
RSS/FD acceptance. Configurable/negotiated timeout classes, retry jitter, agent
online startup, relay bootstrap, desktop/media deadlines, local-disconnect
cancellation while TCP open is pending, global fairness and production network
qualification remain open. The `ssh` command remains a transport forward to an
SSH service; a Rust SSH client/server is separate W5 work. See the
[receipt](reports/rds-client-lifecycle-20260925.md).
