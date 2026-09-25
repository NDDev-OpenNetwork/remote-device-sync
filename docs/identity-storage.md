# Persistent endpoint identity

Scope: W2.4 stable local identity/runtime ownership and the W3.2 persistent
owned-relay identity. See [local sessions](local-sessions.md) for manager use
and [relay runtime](relay-runtime.md) for relay composition. Enrollment and key
rotation remain separate work.

`rds-net::load_or_create_key` is shared by both transport backends. The former
`backends::iroh` path remains a re-export. The raw **32-byte Ed25519 seed format**
and default `XDG_CONFIG_HOME`/`HOME` path are unchanged. The return error is now
the public typed `KeyStoreError`, rather than `anyhow::Error`; callers using
`?` into anyhow still work, but callers naming the old result type must adapt.
The pure local `rds id` command uses this reader. Network-owning binaries use
`acquire_key` on a blocking worker after endpoint configuration preflight.

## Runtime ownership

`acquire_key` keeps the validated seed descriptor from the same creation/read
transaction and takes a **nonblocking exclusive lock on that file inode**.
Contention returns typed `KeyStoreError::InUse`, before network bind. The
`KeyOwner` guard retains the descriptor and key until shutdown. It is neither
Clone nor Debug. Read-only `0400` seeds remain supported and unchanged. Pure
readers do not take the runtime lock, so `rds id` works while the agent runs.
Different keys in the same parent can have independent runtime owners: the
short creation lock still belongs to the parent directory, while the lifetime
lock belongs to each seed. No PID file, stale lock deletion or helper process
is needed. OS process termination releases the lock, including SIGKILL.

The agent retains its owner through endpoint/control/admin shutdown. Explicit
direct CLI commands do likewise and now await endpoint close on operation
errors as well as success. The owned relay acquires ownership during initialize
and transfers it to state retained by its runner and connection tasks; canceling
a consuming shutdown cannot release the key ahead of asynchronous cleanup.
Library callers using `KeyOwner::secret_key()` must retain the owner until all
derived endpoints close. Low-level in-memory-key bind APIs remain available for
fixtures and embedding; they do not implicitly open or own a key file.

This is a cooperative **local inode** guarantee, not distributed identity
fencing. All runtime binaries must be upgraded; old binaries do not participate.
Copying a seed to another file/host, manually replacing/unlinking an active key,
or using a lock-unaware library consumer is outside the guarantee. Parent aliases
that resolve to the same validated inode still contend; hardlinked key files are
refused by storage validation. Do not rotate identity by editing a running key.
Network filesystems, ACLs and native macOS need separate qualification.

The primitive follows [Rust File::try_lock](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)
and [Apple flock](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/man/man2/flock.2).

## Trust and file acceptance

Configured path ancestors and processes with the same OS identity are trusted.
The configured parent may be a symlink; opening it pins the actual directory
for all later operations. Replacing its pathname cannot redirect the running
transaction. This does not sandbox another process with the same credentials.

The immediate parent must belong to the effective user and must not be writable
by group or others. Existing parent permissions are preserved; an ordinary
owned `0755` configuration directory is accepted. Missing directories are
created with mode `0700`, and the parent of each new directory is synchronized.
The pre-existing ancestor hierarchy is assumed to be durably provisioned.
If a restrictive umask prevents opening a newly created directory, cleanup
attempts to remove only that still-empty creation; existing directories are
never changed or removed.

Final key files must be owned regular files with one link and POSIX mode
`0600` or `0400`. Final symlinks, additional hard links, special files,
nonprivate modes and malformed lengths fail without replacement. Reads are
limited to **33 bytes**, accepting exactly 32. Nonblocking, no-follow opens
prevent a FIFO or final symlink from bypassing those checks. Only `ENOENT`
means a missing file; read/permission errors never select key generation.
These are POSIX owner/mode checks, not an extended-ACL audit.

## Creation and recovery

The parent directory reserves `.rds-key-transaction.lock` (the version marker)
and `.rds-key-transaction.pending`. Applications must not use the `.rds-key-`
namespace for keys or unrelated data. The advisory lock is on the **opened
parent directory inode**, before any marker or seed is created. All key names
and filesystem case/Unicode aliases in that directory share it. The directory
descriptor remains pinned; its owner explicitly unlocks even if a temporary
fork-style descriptor alias remains open. Unsupported directory locking returns
an error, with no alternate unlocked path. Lock contention is retried for
**two seconds**, then returns `Busy`. This is
a lock-wait bound, not a deadline on underlying filesystem operations.
`Busy` is a retriable result without an identity: callers can retry after the
other transaction ends. Even a small burst of simultaneous CLI starts may
reach this bound on a busy filesystem; successful calls and later retries
must converge on the same persisted key.

The reserved marker file contains a bounded version, synchronized before any pending
seed is created. An interrupted marker prefix can be completed only if no
pending file exists. Unknown markers or unclaimed pending state fail closed.
While holding the parent lock, the loader can replace its empty private marker
if a process exited before file permissions were established, provided no pending
file exists. This replaces no lock inode: the parent directory remains locked.
After validating the marker, recovery removes only the exact reserved, private,
singly linked pending file of at most 32 bytes. An empty creation with owner
permissions removed by umask is recognized through no-follow metadata, without
reading it. It never scans prefixes or sweeps user files. A pending symlink,
hard link, special file, oversized file or unreadable nonempty payload is refused.

For a missing identity, the loader exclusively creates the pending file,
writes the complete seed, synchronizes it, then atomically renames it with
**no replacement** into the configured key name. The directory is synchronized
before success. Linux uses `RENAME_NOREPLACE`; the rustix Apple backend maps
the same operation to `RENAME_EXCL`. Unsupported filesystem operations return
an error; there is no overwrite fallback. Inode comparisons additionally
reject filesystem aliases of the lock or pending file.
The loader sets `0600` on its exclusively created marker and pending file
descriptors, independent of umask. It does not chmod any existing key or marker.
This prevents returning an identity whose newly created seed cannot be read
on the next invocation.

If an older creator that does not honor this lock wins publication, the loader
validates and reuses its complete private key, or returns its error. It never
replaces that winner. Mixed-version readers can still observe a partial file
written directly by an older creator; upgrade all creators for the new
transaction guarantee.

Before publication, process exit leaves at most one owned pending file. The
next invocation recovers it under the directory lock, including when a different
key name is requested. After publication the final key is complete and stable;
the next load synchronizes the key and directory before returning it. Returned
errors attempt scoped pending cleanup; failed cleanup is recovered on retry.
Changing transaction metadata while users are active is unsupported.
Deleting a final key explicitly permits creation of a new
identity; this is not an automatic rotation or recovery workflow.

## Qualification boundary

The implementation uses safe Rust and the existing rustix dependency, enabling
its `process` feature for effective-user checks. No helper executable, new
package, credential export or unsafe block is introduced. Persistent identity
on unsupported targets fails explicitly instead of using an overwriting writer.

Tests cover independent-process contention, actual CLI `id` contention, existing
raw-format keys, file type/mode/link refusal, bounded lock wait, held-directory
replacement, legacy-creator races, marker recovery and returned-error/process-exit
injection at seven transaction boundaries, including exits before permission
initialization with a restrictive umask. Test keys and paths are temporary.
Native macOS/case-folding filesystems, extended ACLs, physical power loss,
filesystem exhaustion and network filesystems need separate qualification.
The FIFO fixture uses Linux's `mknodat`; the socket/directory fixture compiles
on both supported Unix targets.
File/parent synchronization and process-exit tests do not establish those claims.

The locking primitive follows the descriptor ownership rules in the
[Linux flock reference](https://man7.org/linux/man-pages/man2/flock.2.html) and
[Apple flock reference](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/man/man2/flock.2).
These references do not replace native filesystem qualification.
