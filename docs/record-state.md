# Directory record storage

Status: W1.5 transactional foundation, not a completed directory acceptance
gate. Signed record/delete ordering still uses `issued_at`; explicit publisher
revisions, exact retry, bounded validity, expiry GC and admission fairness are
the next steps in [the remediation plan](remediation-plan.md).

## Transaction and recovery contract

`RecordStore` owns the interface. `MemoryStore` serializes mutations under one
write lock. `FileStore` uses redb 4.3, an embedded Rust library, with no database
daemon, command wrapper or SQL runtime. A transaction updates the signed record
or delete tombstone and catalog metadata together. Deletion preserves its
ordering floor; it does not remove the identity's history.

The file store holds one process-wide mutex through the database's immediate
durability commit and a separate atomic generation-anchor commit. Only after
both commits can readers observe the update or a successful mutation return.
Unexpected storage/validation errors poison that instance, requiring reopen;
semantic stale writes and capacity refusal do not poison it.

On open, validate every bounded row's signature/key, metadata counts, database
identity and anchor. Missing initialized files, corruption, rollback or unknown
files cause an error, never an empty-store fallback. A database exactly one
generation ahead of its anchor represents an interrupted, unacknowledged
mutation: validate it and finish its anchor. A database behind the anchor is
refused, including if the database engine repaired to an older committed root.
An unacknowledged operation may therefore be present after recovery; callers
must not assume that a lost reply means no mutation occurred.

The anchor detects accidental corruption and rollback of the database alone.
It cannot detect restoration of the entire trusted state directory, or an
administrator deliberately rewriting both files. External GDS anchoring remains
W4. Process exits and injected I/O faults are tested; physical power loss and
filesystem/device failure qualification are outstanding.

## Filesystem ownership and budgets

The configured ancestors and OS identity are trusted. The final directory is
opened without following a symlink and protected with mode 0700. Files are
created mode 0600; descriptor-relative opens reject symlinks and nonregular or
multiply linked files. Exclusive OS locks cover the directory owner and the
database descriptor for their lifetimes. Unsupported locking is an error.
redb receives that descriptor through an owned backend; it never opens a path.

Files are `records.redb`, `records.anchor` and `records.lock`. The lock's marker
distinguishes initialized state from a new directory. Atomic anchor staging
uses exclusively created `.policy-*.tmp` names shared with the policy-state
primitive; unknown orphans are left untouched. More than 129 directory entries
requires maintenance. Never remove locks, anchors or apparent leftovers while
a service is running.

Initial fixed limits are 4096 remembered identities (including tombstones),
256 KiB per serialized cell, 256 MiB database file and a 16 MiB database cache.
The backend checks growth and offset arithmetic before writing. These are
storage safety bounds, not completed enrollment quotas or total process-memory
bounds. Exhaustion can deny new identities; expiry GC and protected renewal
capacity are pending. `len()` counts stored nondeleted records, not fresh peers.

## Migration and operations

The previous per-key JSON directory is not accepted or automatically imported.
Unknown legacy files are preserved and startup refuses them. There is no
migration command in this patch. Do not delete the legacy directory or start
an empty replacement to bypass this refusal: that would discard replay history.
Keep deployment on its existing version until the versioned-record issuer and
explicit migration procedure land in W1.5. Synthetic tests use new isolated
directories only.

Back up the entire directory while the service is stopped; a live copy can
mix database and anchor generations. Recovery requires the matching complete
state and independently verified GDS history when replay floors might regress.
Do not repair by deleting an anchor. Disk-full or sync failures close the store;
restore capacity, preserve evidence, then reopen and verify recovery.

## Dependency decision

redb supplies copy-on-write transactions and crash recovery in Rust, replacing
the old multi-file read/compare/write sequence. Implementing our own database
would add page integrity, reclamation and recovery obligations to RDS. The
adapter stays behind `RecordStore`; wire semantics, identity policy, limits and
the generation anchor remain ours. Version 4.3.0 is locked, MIT OR Apache-2.0,
MSRV 1.90; the workspace compiler is newer. Its only dependency added to this
workspace graph is redb itself (libc was already locked). No experimental
features are enabled. This is not a dependency-security audit.

Upstream references: [design](https://github.com/cberner/redb/blob/master/docs/design.md),
[custom storage backend](https://docs.rs/redb/4.3.0/redb/trait.StorageBackend.html).
Immediate durability and commit-error behavior were also checked in the locked
4.3.0 `transactions.rs` source. Native macOS execution remains outstanding.
