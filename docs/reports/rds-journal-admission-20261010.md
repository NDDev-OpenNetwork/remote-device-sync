# Journal preparation cancellation and cleanup — 2026-10-10

Scope: W1.9/W1.10 preparation and the bounded-admission portion of W8.2.
The shared control reader already published cancellation, but `receive` called
`Journal::open` without passing any stop signal. Preparation could keep reading
parts or destination data and holding both receive locks after its caller left.

Two regressions failed before the cancellation wiring: an already canceled
request created state, and interrupted preparation did not return `Interrupted`.
The engine now combines peer termination, caller cancellation and an async-drop
guard. Checks run before filesystem preparation, between catalog entries and
between content chunks. A running syscall still finishes; verified parts remain
resumable and locks release with the blocking operation's owned journal.

Cleanup now streams names through pinned directory handles and shares a
4096-entry work limit across catalog siblings and their parts. Unknown names
consume work but are never deleted. A separate regression failed with the limit
removed: cleanup exhausted the whole journal rather than preserving metadata
and remaining parts for another pass. Deferring cleanup does not reject a new
receive. Filesystem order is unspecified, so a repeatedly encountered foreign
prefix can defer later journals indefinitely; this is not fair background GC.

The implementation follows [rustix directory iteration](https://docs.rs/rustix/1.1.5/rustix/fs/struct.Dir.html),
[directory stream semantics](https://www.man7.org/linux/man-pages/man3/readdir.3.html)
and [Tokio blocking-task cancellation limits](https://docs.rs/tokio/1.48.0/tokio/task/fn.spawn_blocking.html).
The work limit is an RDS admission policy, not a standards requirement or a
wall-clock deadline. No unsafe code, wire/state migration, dependency version or
authorization change was introduced. Single-link/no-follow cleanup checks remain.

## Verification

At clean `119d122295280efc23417c9a656d296f4bb2f5da`, the registered
`w8-journal-admission` checkpoint passed formatting, strict workspace Clippy,
workspace tests and the separate sync suite: 1,049 passing test executions,
zero failures and two explicitly ignored checks. The sync library has 56 tests,
including nine added cases for cancellation, queued work, interrupted cleanup,
part rescan, destination reuse and a wide foreign catalog. Existing returned-error
and process-exit recovery checks also pass; these do not simulate physical power
loss or prove behavior on a genuinely full disk.

The [release benchmark](bench-w8-journal-admission.md) and
[raw source/digest-bound JSON](bench-w8-journal-admission.json) contain 100 samples
per case on macOS arm64. A verified 4 MiB/16-chunk resume measured p95 24.578 ms;
admission beside 8,192 retained foreign entries measured p95 20.792 ms;
pre-canceled admission measured p95 2 µs and created no directory. Setup is
excluded and the cache is warm. These are local observations, not a speedup,
network result or hardware-independent latency guarantee.

PR147's first dependency job failed before running cargo-deny: Docker Hub refused
the action's image pull with HTTP 429, including a separately retained retry.
The shared workflow is being repaired separately to execute the same pinned
tool natively. Do not describe those attempts as successful dependency scans.

Physical/native installed acceptance, quotas and disk-space reservation,
destination-edit preconditions, fair collection, the large-file interruption
campaign and recursive/two-way apply remain open. This checkpoint closes only
the bounded preparation increment; deployed artifacts and consumer pins require
their own qualification and reconciliation.
