# Directory synchronization foundation

The advertised Sync service still transfers one regular file at a time.
The directory APIs are local scan, wire-model and dry-run planning primitives;
they do not authorize or execute recursive filesystem changes.

## Scan contract

`scan_path_with_limits` opens a trusted configured root once and traverses
single names relative to held directory descriptors. It never follows symlinks
and rejects special files and noncanonical/non-UTF-8 paths. Symlink targets are
data. The `.rds-sync` namespace is excluded at every depth because each
destination parent can own private receive state; directory aliases of the
opened private state are excluded by inode identity as well. The manifest
validator continues to reject journal paths from a peer.

Global entry and metadata reservations occur while listing names, before
allocating their copies. A caller can narrow `ScanLimits`; no value can raise
the compiled limits. Nesting bounds descriptors and stack depth. Regular files
are read through one 64 KiB BLAKE3 buffer: chunk boundaries are unnecessary
for a content root. Cancellation is checked during enumeration and before each
read. Already executing syscalls cannot be interrupted.

The scanner compares directory/entry identities and size, mtime and ctime
around observed reads. This detects many concurrent mutations; it is **not an
atomic filesystem snapshot**. A writer may change a previously read file later.
The configured root's ancestors and processes with the same OS identity remain
trusted, as in [single-file confinement](sync-journal.md). A future destructive
apply must use ownership, per-operation preconditions and reconciliation; a
successful scan alone is not permission to delete files.

## Snapshot wire contract

The versioned header carries a content root, exact count and exact path/target
byte total. Parts are strictly ordered and bounded to 32 entries and 48 KiB.
The assembler rejects an invalid part without advancing retained entries or
metadata counters and re-verifies hierarchy and content root at finish.

`snapshot_part_iter` yields borrowed slices. Entries are sized without payload
allocation; each entry is measured at most twice when a byte boundary is hit.
Its postcard bytes are identical to `DirectorySnapshotPart`. The owned
`snapshot_parts` convenience method copies entries only when ownership is
requested. Neither form changes the single-file route or admission policy.

## Reconcile contract

The one-way planner produces deterministic `Put`, `Replace`, exact-identity
`Move`, and `Delete` intents. `Keep` retains destination-only entries, so a
rename becomes a copy intent; `Delete` can use a move. Deletion records carry
the source revision and previous entry. Directory replacements and directory
moves without a subtree identity are refused. Created parents precede new
children and removals run children first.

Validation is explicit:

- `verify` checks structural order, budgets, policy, duplicate targets and
  move sources. It does not prove that supplied entries exist.
- `project_destination` checks the destination revision and exact entry
  preconditions, then returns a validated predicted manifest without I/O.
- `check_inputs` requires the submitted plan to equal the deterministic plan
  derived from both actual manifests. A BLAKE3 revision is identity, not an
  authorization signature.

Three-way conflict preview preserves divergent versions as records and also
reports a removed/replaced directory when the other side changed a descendant,
including a new descendant absent in the base. These records do not choose a
winner or persist file versions.

## Remaining gates and evidence

Journal-backed apply, durable tombstone lifecycle, per-operation publication
barriers, watch/reconcile, metadata/alias policy, grants, recursive routes and
real Linux/macOS destructive acceptance remain open. No capability state moves
from these primitives alone.

`scripts/checkpoint.sh w8-directory-foundation` runs the workspace bars, the
directory contract matrix and the release benchmark. It qualifies only this
foundation. The benchmark checks identical framed wire payloads for every
sample and compares the preserved `cc7cc8cc` cloning algorithm with streaming
borrowed parts. It measures synthetic wall time, not installed RDS CPU.
