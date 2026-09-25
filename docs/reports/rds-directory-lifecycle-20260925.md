# Directory ownership and shutdown — 2026-09-25

Scope: W2.5/W2.6 and W3.2 runtime prerequisites, base `7980cca`.
Linux loopback and temporary catalogs only; no deployed service was restarted.

Source review found that Directory Drop aborted the listener runner, while a
request timeout detached its blocking route handle. The existing semaphore
already retained the worker budget until completion; the missing boundary was
an API that could own and await all started storage work during shutdown.

The directory now shares three bounded task registries with its runner:
connections, blocking requests and one maintenance job. Admission and sealing
use the same short lock. Close seals all groups, stops acceptance, cancels
network tasks and queued blocking jobs, and joins running storage operations.
A canceled waiter retains the runner handle. If the runner fails, close retains
its outcome and joins the shared groups before reporting it. The fallback is
serialized and remains correct when canceled after consuming the runner result.
See the [lifecycle contract](../directory-lifecycle.md).

The server awaits relay and directory shutdown together. Relay startup failure
also awaits directory close. Relay allowlist and TLS flag-shape checks precede
catalog creation. Unix signal handlers are installed before final readiness,
so the real SIGTERM fixture needs no sleep to avoid a registration race.
Full certificate and server-state preflight remains a separate task.

Focused discovery tests passed **60 tests** with one existing ignored case;
real server binary tests passed **3 tests**. New coverage uses actual HTTP/TLS
sockets and explicitly released blocking jobs to exercise request timeout,
canceled/concurrent/repeated close, Drop cleanup, maintenance, worker panic,
runner panic and queued work cancellation. Weak store references and listener
rebinding establish release after close. Binary fixtures verify SIGTERM,
reopening the same catalog, occupied relay port and invalid allowlist refusal.
Only test-owned child processes receive signals.

Final formatting and default/X11/all-feature workspace Clippy passed.
Workspace: **400 tests across 70 targets**, 1 existing ignored test.
All-feature net/relay/agent/CLI: **194 tests across 39 targets**.
Commands, durations and source hashes are in the
[machine-readable receipt](rds-directory-lifecycle-20260925-data.json).
The server test uses the workspace's existing rustix package to signal its
children; no new package, unsafe code or runtime helper executable was added.
`cargo-deny` remains unavailable locally.

Already-running filesystem calls cannot be forcibly canceled: close waits for
them, without a fixed storage deadline. Drop needs the executor to keep running.
Startup policy work before `serve` returns, native macOS, deployment, arbitrary
flood resistance and overall owned-runtime readiness are not qualified here.
No remediation wave is closed.
