# Receive control supervision — 2026-10-10

Status: focused regressions and local checkpoint passed; overall platform
qualification held by the retained control-latency failure, W1.9/W8.1.

Main CI run 38006066964 failed the new path-scope regression while waiting for
the server to stop, after its Need bitmap had arrived. The fixture writes a
raw Cancel on a legacy stream, whereas the legacy sender maps cancellation to
Refuse. Its service budget and observer timeout were both five seconds, so
expiry could masquerade as prompt cancellation and could race the observer.

Source review also found two production boundaries to verify: receive ignores
control messages in the legacy data phase, and the v2 cancellation listener
applies a five-minute control-frame timeout even while data streams progress.
Queued assembly needs the same async-drop projection already used for journal
preparation, so an abandoned waiter cannot publish later from queued work.

1. Use valid legacy refusal in the scope test and separate the long service
   budget from the unchanged five-second observation bound.
2. Prove refusal/EOF wake a receiver waiting for data on both transports.
3. Prove with paused time that quiet control during a data phase is not itself
   a stall. Data I/O and the absolute transfer budget remain bounded.
4. Prove dropping a queued assembly waiter retains the old destination and
   verified parts, and releases receive ownership.
5. Supervise terminal control messages for both wire profiles, propagate every
   receive abort to blocking work, then run full sync/workspace checks and a
   registered checkpoint. No timeout increase or wire-format change.

This investigation is separate from the retained impaired control RTT failure.
No native candidate is promoted merely because a new test run succeeds.

Reference review: Tokio's pinned [spawn_blocking contract](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html)
requires cooperative cancellation after work starts; dropping its async waiter
does not stop it. [RFC 9000 §2](https://www.rfc-editor.org/rfc/rfc9000.html#section-2)
defines independent QUIC streams, so silence on control is not evidence that
the data streams have stalled. The existing per-data-I/O and absolute session
deadlines remain the bounds for this phase. Reviewed on 2026-10-10 against the
actual lockfile; this is not a claim that all referenced library material was
already published by the requested 2026-09-26 historical cutoff.

The source review also found the same independent control-idle deadline in
`push_chunks`; both sender and receiver now use the shared data-phase wait.
Tokio's pinned [bounded receiver contract](https://docs.rs/tokio/1.53.1/src/tokio/sync/mpsc/bounded.rs.html#200-205)
guarantees that losing a `select` branch does not consume a control message.

## Reproduction and implementation

The baseline is `fe27359` plus test fixtures and behavior-preserving extraction
of the assembly closure and data-phase wait. All five expected failures appeared:
legacy Refuse and FIN failed to wake collection; the corrected path-scope fixture
also timed out; paused time exposed the control-idle deadline; and queued assembly
replaced the old destination after its async waiter was dropped. The remaining
120 tests passed. The original failed main CI is retained as
[run 38006066964](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38006066964).

Both wire profiles now supervise control during collection/production. Refuse is
terminal in the owned reader and projects its stop flag before consumption.
Data-phase supervision has no independent idle deadline; actual data reads/writes
and the absolute transfer budget retain their existing limits. Journal assembly
shares preparation's owned cancellation guard, covering all async-drop paths.
No wire variant, protocol version, runtime dependency or timeout constant changed.
The only dependency-feature addition is Tokio `test-util` for deterministic time.

The corrected full `cargo test --locked -p rds-sync --no-fail-fast` run passed
126 tests, zero failed/ignored, including real Iroh/Noq FIN/refusal, both profiles'
reader stop projection, v2 typed cancellation, loss/resume and normal completion.
The baseline had 125 tests; the additional reader-projection regression was added
with the fix. The five-second cancellation observation bound is unchanged; the
fixture's 60-second service budget prevents expiry from masquerading as success.

Cancellation remains cooperative: a syscall already running cannot be interrupted,
and a rename/durability sequence that has begun may commit despite cancellation.
The queued-assembly regression proves the destination and verified resume parts
are retained and receive ownership is released before the replacement starts.
Physical cancellation, power-loss behavior and installed acceptance remain open.

## Checkpoint and platform review

The registered `w1-sync-supervision` gate completed at clean `89db733`: fmt,
workspace strict Clippy, workspace tests and focused sync checks passed, totaling
1,072 test executions, zero failures and two explicit native-feature skips.
The benchmark ran 100 samples per case: p95 verified 4 MiB resume 27.104 ms,
equal-content alternate destination 18.075 ms, pre-canceled admission 8 µs and
admission beside 8,192 retained foreign names 24.985 ms. These are local smoke
measurements, not a comparative performance or power-loss claim. The generated
checkpoint extended the existing intact receipt chain to 15 entries. An earlier
attempt completed the same test count but was interrupted during benchmark
compilation; it has no terminal gate-success receipt and is retained separately.

[PR150 CI run 38015685163](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38015685163)
passed the default macOS workspace, including all sync regressions, then failed
in the desktop-feature lane: 601 control probes yielded RTT p50 164 ms,
p95 411 ms and p99 584 ms against the unchanged 400 ms p95 limit. The synthetic
frame cohort had 624 arrivals, p95 190 ms and queue p95 1 ms. This is a recurrence
of the separate latency investigation, not a successful overall CI run. No
rerun, timeout increase or local passing result is used to retire that failure.
Further platform results must be recorded before source/native promotion.
