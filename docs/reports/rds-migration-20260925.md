# Format-2 record migration receipt — 2026-09-25

Scope: W1.5, based on `02d8646`. Synthetic local Linux evidence only. No estate
directory was converted, no live configuration changed and no deployment gate
was closed. The complete operation and remaining cutover conditions are in
[record-migration-plan.md](../record-migration-plan.md).

## Implemented behavior

`rds-server migrate-v2` runs an explicit offline import into a new sibling.
It holds the existing source lock and a read-only database descriptor, copies
the bounded database into private work state, verifies historical format-2
metadata and every signed record/delete, then commits retained revision floors
to a new format-3 database. The original database, anchor and initialized marker
remain byte-identical. The engine can repair only the copy. Positive revisions,
exact mutation digests and deletion meaning survive; old addresses never acquire
a new lease. Publishers advance through the existing 410/durable-successor path.

The verified staging catalog retains ownership through receipt synchronization,
pending-marker removal, exclusive directory rename and both parent syncs. Existing
destinations, including ones created during verification, are not overwritten.
Interrupted attempts retain identifiable private audit state and no partially
populated active destination. A failure after rename can leave the complete new
directory; absence of CLI success must not be interpreted as permission to erase it.
The receipt describes preparation and does not authorize configuration cutover.

No dependencies or runtime helper programs were added. Linux and macOS use the
existing safe rustix filesystem bindings; native macOS execution remains pending.

## Regression and interruption coverage

The format-2 fixture uses the historical `0dea767` table names, postcard metadata
layout and record/delete enum encoding. Cases cover:

- Fresh record, deletion, expired signed content, maximum revision and an
  initialized empty catalog. Reads of imported records return expired; exact
  retries cannot rearm leases and lower revisions remain refused. Successors
  publish normally and retain exact retry behavior.
- Coherent and one-ahead source generations, behind-anchor recovery, wider
  generation gaps, wrong database identity, conflicting counts and old/new
  unsupported format tags.
- Forged signature, wrong signed key, trailing cell/metadata bytes, oversized
  cell/file, corrupt database/anchor, unknown table/multimap/file and extra or
  inconsistent metadata. Source bytes are compared after refusal.
- Source owner and database-lock contention, source/destination aliases,
  nested destinations, final symlinks, multiply linked files, ambiguous trailing
  path components and a destination created during validation.
- Returned errors and abrupt child-process exit at 12 checkpoints: before copy,
  partial copy, after copy, after source validation, before/after destination
  commit, after anchor, before/after receipt, after pending-marker removal,
  after rename and after parent synchronization. Before rename the requested
  destination stays absent. Incomplete staging cannot pass normal FileStore
  open; a fully prepared retained staging catalog keeps all floors. After rename
  the complete catalog reopens. Original source bytes remain identical.
- Real directory HTTP: migrated publication receives 410, the durable issuer
  allocates revision 2, a raw TCP request commits while its reply is never read,
  and the restarted issuer retries identical signed bytes successfully. Directory
  teardown drains its owned store references before the final database reopen.

Development checks initially caught two fixture defects: regenerating an expected
signed envelope across a second boundary changed its digest, and immediate reopen
after aborting the directory raced its asynchronous ownership teardown. The
fixtures now retain the original envelopes and wait for ownership release under
a bounded deadline. Production expiry or locking behavior was not relaxed.

## Local validation and capacity

Passed: `cargo fmt --check`; default, X11 and all-feature workspace Clippy with
`-D warnings`; **256 workspace tests across 49 targets**; **59 all-feature tests
across 18 targets** for `rds-net`, `rds-agent` and `rds-relay`. The ordinary workspace
run explicitly skips the expensive capacity test; that test was run separately
and passed. `cargo-deny` is not installed. No wave-close checkpoint is claimed.

The separately invoked Rust test imported **4096 signed identities**, each with
32 direct addresses and eight maximum-length relay origins, and compared every
retained revision/digest/kind and the original source bytes. Migration took
**67.658 seconds** in the ordinary unoptimized Cargo test profile. Source database
length was **33,771,520 bytes**; the retired-floor destination was **1,056,768 bytes**.
This is a single offline conversion timing, excluding fixture construction and
the post-migration assertion pass, not a connection-latency or production-load
percentile. The complete test took 148.04 seconds. A 4097th publication remained
refused after conversion. See [machine-readable measurements and check results](rds-migration-20260925-data.json)
for commands, durations, toolchain and binary fingerprint. Private logs retain
the detailed results; no live records or identities enter this public report.

## Limits and remaining work

The conversion retains every identity slot and does not prove current GDS
membership. Format-1 timestamp state and per-key JSON need an authenticated
inventory/retirement cutover; timestamps cannot become publisher revisions.
Whole-directory rollback still requires an external GDS anchor. Missing publisher
history and an exhausted revision remain explicit repair conditions.

Native macOS, physical power-loss/storage exhaustion, release startup/load
profiling and deployment successor/membership reconciliation remain open.
Artifacts are retained deliberately; automatic resume and cleanup are not
provided. The two database files each retain the 256 MiB bound, so an operation
may require 512 MiB plus small metadata in additional space. The ordinary source
database is preserved separately. No performance target is inferred from the
functional tests or the earlier directory renewal measurement.
