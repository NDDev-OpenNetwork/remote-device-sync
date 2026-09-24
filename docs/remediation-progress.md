# RDS remediation execution

Started: 2026-09-24. Plan: [remediation-plan.md](remediation-plan.md).
Audit baseline: `87e7aeabaad861b0d3c63f6641c91539be19a3e0`.
Audit/plan snapshot: `6772d4a`. Historical findings stay in the dated audit;
this file records implementation progress rather than rewriting that evidence.

## Current state

All waves remain open. W1.1/W1.2 and W1.6–W1.8 passed the local Linux check matrix; the
other tasks remain planned unless listed below. No deployment or owned-backend
promotion has occurred. Native macOS checks still require their platform lane.

| Task | State | Evidence / remaining scope |
|---|---|---|
| W0.1 | Partial | R01/R10 are agent regressions; R03/R04 are journal regressions, with failures observed before fixing. R05 is covered by planted-link and directory-substitution tests. R02/R06–R09 and desktop body cancellation still need their owning fixes/tests. |
| W1.1 | Implemented; Linux checks passed | Denylist replacement retains its value without observers; atomic modification preserves concurrent revocations. Subscribe-before-check and initial watchdog snapshot check remove missed-update windows. Durable feed freshness remains W1.4. |
| W1.2 | Implemented; Linux checks passed | One authorization state owns admission, replay reservation and watchdog. ACK failure/cancellation closes the connection and releases the grant. Service admission checks live validity/revocation. Connection future teardown runs RAII cleanup. |
| W1.6 | Implemented; Linux checks passed | Reused bytes are verified and stored before `have`; edits, insertions, deletions, repeated chunks and destination removal/restart are tested. |
| W1.7 | Implemented; Linux checks passed | Exclusive random staging names and RAII cleanup preserve ordinary/link siblings and colliding names; failed assembly retains the old file. |
| W1.8 | Implemented; Linux checks passed | Directory-relative no-follow journal/destination I/O and a held source file replace path-check-then-open. Link planting and substitutions after open are tested. Native macOS verification remains pending. |
| W1.10 | Partial; Linux checks passed | Persistent per-root receive lock covers processes and filesystem name aliases; parts and assembly use sync/rename/parent-sync ordering. Abrupt-process-exit regression added. Power-loss, every commit boundary, orphan collection and native macOS durability still need qualification. |

## Authorization change

`rds-agent::authz` owns Pending → Authorizing → Granted → Closed. Services
remain pending while an authorization response is being written. The watcher
is installed before that I/O; a 15-second response deadline bounds it.
Duplicate Authz cannot replace a pending or granted lease. Failed verification,
response write, interrupted commit and canceled admission close the connection.
Closing the connection cannot be undone by a late admission task.

The watchdog checks the current denylist immediately, then observes updates
and wall-clock expiry. Each service also checks policy directly so correctness
does not depend only on watchdog scheduling. Replay reservations and watchdogs
are released through owned drop guards, including cancellation of the
connection service future.

Regression evidence before implementation:

- `revocations_survive_without_watchers`: failed; initial revoked ID was lost.
- `concurrent_revocations_are_not_lost`: failed; fewer than 16 concurrent IDs survived.
- `failed_authz_reply_closes_connection_and_releases_grant`: failed; stopped
  reply retained the connection beyond the test's bounded observation window.

Additional coverage exercises simultaneous admission ownership, a snapshot
already present before watchdog startup, admission cancellation, immediate
service-scope revocation/expiry and reservation cleanup.

Local validation on 2026-09-24: `cargo fmt --check`; workspace clippy with
default features, `rds-desktop/x11`, and all features (all with `-D warnings`);
`cargo test --workspace`; `cargo test -p rds-agent --all-features` (5 unit and
15 integration tests). All passed. Native macOS execution is still pending.
`cargo-deny` is not installed locally. No wave checkpoint is claimed: these
changes do not close W1 or replace historical benchmark evidence.

The signed revocation feed still needs durable anti-rollback/freshness/offline
policy (W1.4); this patch does not close all of audit A02. Audience binding and
renewable leases remain W2.3. Long-lived service task ownership remains W2.5.

## Sync correctness and filesystem change

Baseline `cargo test -p rds-sync --test journal` failed four regressions:
partial reuse was lost on reopening, repeated reused chunks could not assemble,
and both the ordinary `.rds-part` sibling and its link variants were deleted.
The new journal materializes reused content, including size-changing deltas,
then assembles only verified parts. `fetched` counts network chunks, not reuse.

`confined::Directory` holds directory handles and uses safe `rustix` bindings
for relative no-follow opens, rename and unlink. The private state directory
has mode 0700, including journals created by the earlier implementation.
Staging files use exclusive creation and random 128-bit names; collision
handling never removes someone else's entry. All state reads are length-bounded
and refuse special files, symlinks and multiply linked part files. Cleanup
does not recursively walk arbitrary journal entries.
The journal namespace is reserved regardless of ASCII case; a second check
compares opened directory identities to refuse filesystem case/Unicode aliases
of `.rds-sync` before descending into a peer-requested path.

The configured root's ancestors and the local OS identity are trusted. A held
directory is an inode capability, including after rename; this does not promise
an absolute pathname boundary against a privileged/local actor relocating that
inode. A remote peer cannot select arbitrary roots or enter `.rds-sync`.
Journal and destination directory handles stay pinned throughout I/O; pull
chunk reads use the same opened file as the manifest, with positioned reads
shared across senders.

Receives within one root are serialized by one persistent lock inode. This
intentionally also prevents alias races on case/Unicode-insensitive filesystems
and avoids an unbounded collection of per-filename locks. Separate roots remain
independent. More granular parallel receives require a proven alias model.
Existing parts remain readable after upgrading, but old and new receive
processes must not share a root concurrently: old binaries do not take the
new lock. New staging files have mode 0600; file metadata preservation is W8.5.
The receiver watches control-stream cancellation while collecting chunks;
sender task ownership uses `JoinSet` so cancellation aborts its async children.
Blocking disk jobs can finish their current operation and drain their bounded
queue; fully interruptible scans and absolute budgets remain W1.9/W2.5.

File and parent sync occur before reporting commit. A parent-sync error after
rename reports uncertainty and keeps recovery state; the caller receives no
false `Done`. The process-crash test targets committed parts and lock recovery;
it does not simulate loss of OS caches or certify macOS power-loss durability.
Unknown orphan staging entries are preserved for future owned-state GC (W8).

Dependency rationale: `rustix` supplies thin safe OS calls; `rand` supplies
staging entropy. Both versions were already in `Cargo.lock`. No helper process,
native code block or new dependency version is introduced by the sync runtime.

## Measurement fixture correction

An intermediate workspace run failed the existing 20-second desktop soak:
frame age p95 1319 ms, p99 1657 ms, 835 received frames. Running the **same
compiled test alone**, without changing its thresholds, passed at p95 32 ms,
p99 63 ms and 1110 frames. This indicates sensitivity to the execution context;
it does not establish a unique root cause or certify production latency.

Inspection also found that the supposed clean in-process desktop lane used
default endpoints with public discovery enabled. That fixture now explicitly
binds loopback and disables discovery, so its direct-path claim is enforced.
Impairment remains on the separately configured controlled socket lane. The
150/250 ms p95/p99 budgets are unchanged. The final check matrix passed after this fixture correction and the journal
namespace-alias check: format; workspace clippy in default, X11 and all-feature
configurations; and `cargo test --workspace` (135 tests in 36 targets). The
sync crate contributes 5 unit, 13 journal and 14 end-to-end tests. Native macOS
and power-loss qualification remain open; no entire wave is closed.

## Next sequence

Proceed with W1.3 signed name trust and W1.4 durable
revocation freshness, followed by W1.5 directory transactions and W1.9 wire
binding/accounting. Each change retains
its own failing-before/passing-after evidence. No general CI or long soak is
made a blanket barrier to independent development; actual invariant failures
are repaired and platform evidence is recorded separately.
