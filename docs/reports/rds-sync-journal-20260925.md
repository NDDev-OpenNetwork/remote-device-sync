# Single-file journal recovery — 2026-09-25

Scope: W1.10, with W1.7/W1.8 namespace and overlap corrections. Base `553519d`.
Linux x86_64 local qualification; all data and paths in fixtures are synthetic.
This receipt does not close a remediation wave.

## Findings and changes

Two new integration regressions failed before the fix: overlapping configured
roots could own the same destination simultaneously, and a nested `.rds-sync`
path could target private receive state. A root lock alone did not cover the
former. The old assembly also created untracked random temporary files beside
user data, with no deterministic recovery after a crash.

Every receive now holds its root lock and, when different, its destination
parent's private lock. `.rds-sync` is reserved at every path depth, with both
lexical and directory-inode checks. Assembly stages privately in that parent,
then syncs data, renames and syncs both directories. Metadata and part temporary
names are also private and reserved. Reopen reclaims known temporary regular
single-link files; it refuses links and special files and preserves unknown
entries. Verified committed parts keep the existing on-disk layout.

Cleanup after durable publication syncs each affected directory level. Cleanup
failure is reported as a generic warning rather than falsely changing committed
success into transfer failure. No error before durable publication is treated
as Done. See [the full recovery contract](../sync-journal.md).

## Fault matrix

| Operation | Boundaries |
|---|---|
| Metadata write | Exclusive create, half-body write, body complete, file sync, rename, parent sync |
| Part write | Exclusive create, half-body write, body complete, file sync, rename, parent sync |
| Assembly/publication | Exclusive create, half-body write, body complete/root verified, file sync, rename, destination sync, source sync |
| Cleanup | Part removal, metadata removal, empty parts-directory removal, empty journal removal |

Each of these **23** points runs with an abrupt subprocess exit (code 86, no
destructors) and separately with injected ENOSPC and EACCES errors: **69 cases**.
Assertions check the prior or complete verified destination as appropriate,
root-lock release, committed-part reuse, reserved-temporary recovery, exact
resumed bytes and removal of the completed known journal. The original SIGKILL
mid-receive test remains in the integration suite.

Another 12 combinations cover regular/symlink/hardlink/directory objects at the
three reserved temporary locations. Unknown journal entries and unrelated file
contents must survive. Existing partial reuse, mutated source, repeated chunks,
directory substitutions, planted links and process-lock tests remain active.
Production builds contain no environment-controlled fault injection, and normal
writes are not split into artificial half-bodies.

The first full matrix exposed one old test explicitly allowing nested private
paths. Its expectation was corrected to the new ownership contract; no valid
ordinary destination path was changed by that correction.

## Final checks

Formatting and default/X11/all-feature workspace Clippy passed with warnings
denied. `cargo test --workspace` passed **273 tests across 50 targets**, with the
previously qualified 4096-identity migration capacity case intentionally ignored.
The separate all-feature network/agent/relay lane passed **59 tests across 18
targets**. The sync library and journal integration suite account for 10 and 15
tests respectively; full workspace coverage also includes protocol safety,
impaired transfer, resume and mixed desktop/sync fixtures.

[Machine-readable evidence](rds-sync-journal-20260925-data.json) records command
arguments, exit codes, durations, source hashes and counts. Full logs stay in
private local evidence storage. `cargo-deny` is not installed locally. No wave
closure is claimed and no unregistered checkpoint gate was invoked.

## Limits

- Injected syscall errors and process death do not model real power loss,
  filesystem corruption, controller caches or a physically full disk.
- Native macOS, actual nested mount points and W8's 1 GiB/20-kill campaign were
  not executed here. Dynamic creation of configured-root ancestors has not
  received physical durability qualification.
- Cancellation can still outlive async work through blocking disk operations.
  Concurrent edits/overwrite policy remains W8.2.
- Inactive journals, old random staging entries and unvisited destination
  parents need explicit quota/collection work. The implementation never sweeps
  user data or claims a global disk-usage cap.
- Old receivers must be quiesced when upgrading shared overlapping roots;
  they did not acquire the new common destination-parent lock.
- No dependency, unsafe block, external runtime helper or deployment was added.
  Functional fault counts are not connection-latency measurements.
