# RDS runtime identity ownership and default manager migration

Date: 2026-09-25. Scope: W2.4/W3.2 implementation and Linux regression evidence.
Baseline: `a34ac1f` (opt-in local manager). This is not a wave close, release
qualification, deployment receipt or latency benchmark.

## Result

The agent enables local session control by default in `<key-file>.control`.
Ordinary ticket, ping, info and SSH/TCP forwarding commands use that running
agent endpoint. They neither load a key nor bind UDP, and a missing manager
does not trigger a direct fallback. An explicit control path remains supported.
The agent creates missing private path components after checking their parents;
clients never create them. Same-UID peer authentication and existing filesystem
checks still apply. A dedicated server can explicitly select `--no-control`.

`rds --direct` retains independent operation and requires a persisted identity.
Agent, direct CLI and owned relay now acquire `rds-net::KeyOwner`, an exclusive
nonblocking lock on the validated seed inode. Contention fails before endpoint
bind; `rds id` remains a reader and works while the owner runs. The raw seed,
storage transaction, backend default and remote wire formats are unchanged.
The local IPC version is 2, adding live ticket retrieval; CLI and agent must be
upgraded together. Ticket output is current address information, not a relay
readiness or peer reachability assertion.

Direct CLI operation errors now await endpoint close before releasing identity.
The agent also closes its endpoint on announcer startup failure. Owned relay
initialization reserves the key and transfers ownership to runner/service state,
so canceling consuming shutdown cannot release the key before task cleanup.
No new package/version, external executable or custom unsafe block was added.
CLI now depends directly on the already-used rustix library for nonblocking,
no-follow grant-file opens; the opened descriptor must be a regular file and
reads remain capped at 64 KiB. This closes a reproduced FIFO startup hang.

## Regression scenarios

- Eight independent processes race creation and acquisition of one fresh key:
  exactly one holds runtime ownership. SIGKILL releases it; reopening preserves
  the public identity. Read-only seeds, parent aliases, simultaneous readers and
  independent keys in the same directory retain their intended behavior.
- A real agent process starts using the default control path under a missing
  private parent. A second process with another control directory, and a second
  process using `--no-control`, both fail without announcing an endpoint.
  SIGKILL leaves a stale socket; restart recovers it, changes manager instance,
  preserves the key, and serves a real peer connection. SIGTERM removes the
  owned socket and releases the key.
- A real CLI ping creates a managed session. Six parallel CLI Info processes
  reuse that session and one remote connection without changing its generation.
  Ticket retrieval uses the agent identity, and absent-manager commands fail
  without creating a key. Incompatible direct options fail explicitly.
- A held owner blocks the real direct CLI at runtime while `id` remains usable.
  Once released, direct Ping works again. Existing manager tests retain multiple
  peers, pinned forwarding, half-close integrity, cancellation, credential scope,
  revocation, IPC budgets and filesystem refusal coverage.
- Grant loading rejects FIFOs, directories, final symlinks, malformed JSON and
  oversized files before manager contact or key creation. Both ordinary and
  session-connect command paths exercise the shared reader with a deadline.
- Owned relay initialization excludes another owner. Canceling shutdown during
  drain retains ownership until runner cleanup; subsequent acquisition returns
  the same identity. Shared runtime composition is also exercised by the relay
  and server binary test suites in the feature-enabled lane.

## Validation

Final command outcomes and source/log hashes are recorded in
[machine-readable evidence](rds-identity-migration-20260925-data.json).
Environment: Linux x86_64, Rust/Cargo 1.98.1; Cargo jobs limited to two, one
Cargo process at a time, isolated build artifacts. Test fixtures use synthetic
keys, private temporary directories and loopback sockets. The existing unrelated
agent service is not stopped or reconfigured.

| Check | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | pass |
| `cargo clippy --locked --workspace --all-targets --features rds-desktop/x11 -- -D warnings` | pass |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass |
| `cargo test --locked --workspace` | pass; 489 passed, 2 ignored across 86 result targets |
| `cargo test --locked --workspace --all-features` | pass; 535 passed, 2 ignored across 86 result targets |

The feature-expanded lane overlaps the default tests; totals are not additive. All 214 Rust/Cargo source hashes remained unchanged during the final command sequence.

`cargo-deny` is not installed. A usable Apple SDK/native macOS executor is not
available here; macOS is not reported as passed. No checkpoint command is run
because this increment does not meet a complete registered wave gate.

## Boundaries and next work

The lock is cooperative and local to a seed inode. Old binaries, copied keys on
another inode/host and manual replacement/unlink of an active key are outside
the guarantee. Embedding callers must keep `KeyOwner` until derived endpoints
close. See [identity storage](../identity-storage.md#runtime-ownership).

Desktop and file transfer still require explicit direct mode with a separate
authorized key; their manager APIs are unfinished. Native Rust SSH/PTY,
interactive viewer/focus/input release, native platform capture/input, GDS
enrollment/renewal, recursive sync and observability rollout remain tracked in
the [full remediation plan](../remediation-plan.md). Native macOS, actual
distinct-user rejection, installed-binary upgrades, relay-registration reuse,
FD/RSS budgets and physical network/latency campaigns remain unqualified.

See the [local session contract and remaining sequence](../local-sessions.md).
W2.4 stays partial and every W0–W10 release gate remains open.
