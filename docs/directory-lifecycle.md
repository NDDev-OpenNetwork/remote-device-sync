# Directory task ownership and shutdown

Scope: W2.5/W2.6, a prerequisite for W3.2 server runtime composition. This
contract begins when `rds_discovery::service::serve` returns a `Directory`.

Each directory owns its listener, bounded asynchronous connection tasks, a
bounded registry of blocking request jobs, and one separate maintenance job.
The request registry bounds active and completed-but-unjoined handles by
`Limits::max_workers`; it has no unbounded waiting queue. An HTTP timeout drops
the response waiter, while the running storage job remains owned and charged.
Worker saturation retains the existing HTTP 429 response and empty HEAD body.
A panicked request worker returns a generic 500 when its request is still alive;
its handle is reaped and capacity becomes available again.

Maintenance has separate capacity so request saturation cannot consume its
slot. Only one collection runs at a time. The listener prioritizes shutdown,
completed-task reaping and maintenance scheduling before accepting more sockets.
Those sets are bounded, and no successor collection is scheduled after sealing.

`Directory::close(&self)` first seals all three task registries, then signals the
runner. The runner drops its listener, cancels and joins asynchronous requests,
then joins both blocking groups. All three task registries remain shared
with the Directory owner, including after an abnormal runner exit. Blocking
jobs that have not started are canceled;
already-running filesystem calls cannot be forcibly stopped and are awaited.
There is no claim of a fixed shutdown deadline for a stalled kernel/filesystem.
The caller must keep the Tokio runtime alive while awaiting close.

The runner join handle is retained behind an asynchronous mutex. Canceling one
close waiter does not lose the runner or child handles. Normal cleanup proceeds
in the runner; after a runner failure, another close waiter can resume fallback
joining. Concurrent and repeated close calls converge. The runner outcome is saved before fallback draining, so
cancellation during fallback cannot repoll an already-consumed join handle.
If the runner fails, close still seals and joins the shared connection, request
and maintenance groups, then reports the retained failure. Only one close waiter
polls these fallback groups at a time.

`Directory::wait_stopped(&self)` observes runner termination without requesting
shutdown or joining failure-fallback storage jobs. It retains the result for
subsequent observers and close calls. Canceling an observer leaves a healthy
listener running. Call `close` after observation to seal and join retained work;
observation alone is not a cleanup guarantee. A concurrent close delivers its
stop request before waiting for the runner mutex, so an active observer cannot
prevent shutdown. This allows a composed host to start stopping its other
services before a failed directory's remaining disk jobs finish. Observation
shares the runner mutex with close; if another caller has already begun cleanup,
observation may wait for that caller. The host observes before starting close.

Drop seals admission and signals cleanup, but cannot synchronously join. The
executor must continue running for cleanup to progress. Applications needing a
completion guarantee must call and await close before ending their runtime.
Startup policy bootstrapping before `serve` returns is a separate cancellation
boundary and is not qualified by this contract. A forced process exit is also
outside the guarantee.

`rds-server` starts relay and directory shutdown together and awaits both before
returning. It also initiates both shutdowns if either service runner terminates
unexpectedly, and reports failure even for an unexpected clean exit. A relay
startup error awaits directory close and retains both errors if cleanup fails.
Relay configuration and bounded certificate/registry input preflight precede
identity/catalog creation; see the [runtime contract](relay-runtime.md). On Unix,
SIGINT/SIGTERM handlers are installed before final relay readiness is logged.
Real binary tests use private temporary catalogs, loopback listeners and signals
sent only to test-owned children; they also reopen the same catalog after exit.

The implementation follows Tokio's documented [blocking-job cancellation](https://docs.rs/tokio/1.53.1/tokio/task/struct.JoinHandle.html#method.abort)
and [task-group join](https://docs.rs/tokio/1.53.1/tokio/task/struct.JoinSet.html#method.shutdown)
semantics. It uses existing Rust dependencies, without helper processes or
production fault-injection flags. Native-platform and deployed-service
qualification remain open.
