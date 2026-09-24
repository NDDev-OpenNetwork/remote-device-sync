# Single-file journal ownership and recovery

Status: W1.10 local transaction recovery and overlap exclusion are implemented.
Physical power loss, native macOS durability, inactive-journal quotas and the
large-file acceptance campaign remain open. This is not a two-way sync protocol.

## Ownership

A receive holds the persistent `<root>/.rds-sync/receive.lock` inode until all
blocking work ends. It also holds `<destination-parent>/.rds-sync/receive.lock`
when that is a different directory. Both are nonblocking OS locks; a conflict
fails without waiting. This gives overlapping configured roots a shared owner
for a destination and covers case/Unicode aliases. Locks are never unlinked.
Unrelated roots and destination parents remain independent.

`.rds-sync` is reserved at **every** path depth, including case variants. A
handle-level inode comparison additionally rejects filesystem aliases when
walking source and destination parents. It is not possible to pull or push
another receive's internal files through a nested path. The configured root's
ancestors and same-OS-identity processes remain trusted; pinned directories
continue to refer to their inodes if a local actor renames them.

## Transaction sequence

Verified parts remain at `<root>/.rds-sync/<content-root>/parts/<chunk-hash>`.
Existing journal layout and verified chunk reuse are preserved. Metadata and
part writes use the reserved name `pending` inside their private directory:
exclusive create, write, file sync, rename to the committed name, parent sync.
Only after that sequence does a stored chunk count as present.

Assembly uses `<destination-parent>/.rds-sync/assembly`, mode 0600, created
exclusively under both locks. Every part is re-verified and the assembled root
must match the manifest. After syncing the staging file, rename installs it
through the held destination parent. The destination directory and the private
source directory are both synced before returning success. Staging stays on
the destination filesystem even if the configured root is above a mount point;
native mount-point behavior still needs separate qualification.

Before rename, failures retain the prior destination. After rename, a failure
can leave a complete new destination while reporting an uncertain outcome.
Only successful durable publication allows the engine to emit Done. Async
cancellation cannot revoke an already-running filesystem syscall or started
assembly; reconcile actual content after an uncertain result.

## Recovery and cleanup

Opening a receive under its locks discards known `assembly` and `pending`
temporary names. Only regular, single-link files qualify; symlinks, hard links,
directories and special files fail closed. Committed parts are independently
bounded and hash-verified, so torn metadata does not invent progress. Recovery
does not require trusting the advisory metadata or a partial assembly.

After durable publication, cleanup removes only the manifest's known parts,
metadata and empty owned directories, syncing each directory level. A cleanup
error logs a generic warning and leaves a successful transfer successful.
Reopening reconstructs verified state from remaining parts and/or destination
bytes. Unknown entries are preserved; recursive deletion is never attempted.

Recovery work is bounded by the requested manifest and a fixed number of
temporary names. It does not enumerate historical journals. A crashed assembly
has at most one known temporary inode per destination parent, reclaimed on the
next receive there. Old random `.rds-stage-*` files have no trustworthy ownership
receipt and are preserved. Inactive journals, abandoned destinations, disk quotas
and legacy orphan reclamation remain W8.2. This change must not be treated as a
general garbage collector or a disk-usage cap.

Upgrading agents that share overlapping roots requires quiescing old receives:
older versions did not take the destination-parent lock and cannot enforce the
new overlap contract. Older clients requesting nested `.rds-sync` paths are now
refused before receive state is created; valid file-transfer layouts are unchanged.

## Evidence

[The 2026-09-25 receipt](reports/rds-sync-journal-20260925.md) records before-fix
ownership/namespace failures, returned-error and process-exit matrices, recovery,
confinement and remaining qualification. Error injection covers transaction
boundaries and torn bodies, not actual physical power loss or a real full disk.
