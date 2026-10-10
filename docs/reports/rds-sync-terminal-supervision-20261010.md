# Receive control supervision — 2026-10-10

Status: investigation and regression plan, W1.9/W8.1.

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
