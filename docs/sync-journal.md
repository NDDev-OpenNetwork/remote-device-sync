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

New verified parts live at
`<root>/.rds-sync/v2-<content-root>-<destination-hash>/parts/<chunk-hash>`.
The destination hash is BLAKE3 of the normalized relative path's UTF-8 bytes.
Different destinations never share cached parts merely because their offered
content roots match. Legacy `<content-root>` journals resume only when verified
metadata matches the exact destination, root and size; malformed or unbound
legacy state stays untouched beside the new journal. Metadata and
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

## Receive cancellation

The async chunk sink owns a cancellation guard for its bounded disk queue.
Dropping the sink or canceling its `finish` future requests abort if the blocking
worker has not started, and tells a running worker to stop between stores. The
flag is published before dropping the job sender. Pending chunk stores are not
drained merely because a canceled producer closed its channel. Normal `finish`
keeps the guard alive while the writer drains and returns its journal.

Explicit peer refusal/cancellation and receive errors now retain the sink in
the parent receive owner across control selection. They stop and join that exact
writer under a five-second cleanup budget, disposing of any returned journal
before reporting termination. Canceling a drain waiter cannot discard its join
handle. Cleanup failure retains the original transfer cause and explicitly
reports incomplete cleanup. This closes the store-worker completion race; it
does not forcibly release a lock held by a running syscall, journal scan or
abandoned assembly, or promise synchronous cleanup after an async-waiter drop.

A store already executing remains atomic at its existing filesystem transaction
boundary; cancellation cannot interrupt a filesystem syscall or roll back a
committed part. Journal scan/open and assembly retain their separate cancellation
limits. `Journal::open_cancellable` checks cancellation before filesystem
preparation, between catalog entries, before each saved-part read, and between
destination-reuse chunks. The engine combines peer/control termination, caller
cancellation and an async-waiter drop guard. A queued abandoned open performs no
filesystem mutation; a running syscall completes before the next check. A
canceled open returns `Interrupted`, retains verified parts and releases both
receive locks. The receive lock remains owned for as long as that work needs it; it is
never forcibly removed to let a retry overlap. Immediate reconnection may still
be refused while the previous operation unwinds. Recovery must reconcile actual
verified content and allow bounded convergence, not assume a fixed sleep proves
remote cleanup. See the [queued-store receipt](reports/rds-sync-cancel-20260925.md).

## Recovery and cleanup

Opening a receive under its locks discards known `assembly` and `pending`
temporary names. Only regular, single-link files qualify; symlinks, hard links,
directories and special files fail closed. Committed parts are independently
bounded and hash-verified, so torn metadata does not invent progress. The new
directory name binds the destination even when its advisory metadata is torn;
legacy names require verified metadata for attribution. A partial assembly is
never accepted as a committed destination.

After durable publication, cleanup removes only the manifest's known parts,
metadata and empty owned directories, syncing each directory level. A cleanup
error logs a generic warning and leaves a successful transfer successful.
Reopening reconstructs verified state from remaining parts and/or destination
bytes. Unknown entries are preserved; recursive deletion is never attempted.

Recovery of the offered journal verifies its requested manifest and known
temporary names. Opening also attempts to collect verified superseded journals
with a different content root for the same destination. Both legacy and new
names must agree with verified attribution metadata. This uses a streaming directory visitor and a shared
4096-entry budget across catalog siblings and their parts; unknown names also
consume that budget. Exhausting it defers cleanup and allows the new receive to
proceed. Partial cleanup retains attribution metadata until known parts and
their directory have been removed. Directory order is unspecified: a large
foreign prefix can defer later entries on every pass, so this is not a fair
background collector or an eventual-reclamation guarantee. Cancellation ends
the entire preparation instead of being swallowed as a cleanup warning.

A crashed assembly
has at most one known temporary inode per destination parent, reclaimed on the
next receive there. Old random `.rds-stage-*` files have no trustworthy ownership
receipt and are preserved. Inactive journals, abandoned destinations, disk quotas
and legacy orphan reclamation remain W8.2. This change must not be treated as a
general garbage collector or a disk-usage cap.

Upgrading agents that share overlapping roots requires quiescing old receives:
older versions did not take the destination-parent lock and cannot enforce the
new overlap contract. Do not run an old receiver against state governed by a
new receiver: it still uses the unscoped legacy layout. A downgrade cannot safely
resume a new journal and restores the older selection behavior. Older clients
requesting nested `.rds-sync` paths are refused before receive state is created;
ordinary client wire formats remain unchanged.

## Evidence

[The 2026-09-25 receipt](reports/rds-sync-journal-20260925.md) records before-fix
ownership/namespace failures, returned-error and process-exit matrices, recovery,
confinement and remaining qualification. Error injection covers transaction
boundaries and torn bodies, not actual physical power loss or a real full disk.
