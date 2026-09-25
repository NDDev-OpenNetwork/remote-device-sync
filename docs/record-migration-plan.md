# Record format migration — operation and remaining qualification

Status: format-2 offline conversion is implemented in Rust. Deployment qualification,
native macOS and the timestamp/legacy cutover remain open W1.5 work. Normal format-3
startup still refuses older or ambiguous directories. Never remove its anchor,
initialized marker or old directory to make startup succeed.

## Available offline operation

With the source service stopped and an existing trusted parent directory:

```sh
rds-server migrate-v2 --source /state/records-v2 --destination /state/records-v3
```

Both paths must end in a literal basename and share the same parent inode.
The source must already have mode 0700 and initialized lock/anchor/database files.
The destination must not exist, even as an empty directory or symlink. This
subcommand never starts the directory or relay, and rejects service flags.
It does not change live configuration. Application and migration code use Rust
libraries and OS filesystem operations; no database daemon or conversion program
is invoked.

Source ownership and database locks are held through completion. Original database,
anchor and initialized-marker bytes are never written. The importer copies at
most 256 MiB through a 64 KiB buffer, then lets redb recover **only the copy**.
It verifies the historical format-2 metadata/anchor and every exact signed envelope,
including expired records and tombstones. It refuses unknown tables, excess rows,
oversized cells, extra metadata, trailing bytes, key/signature mismatches and
rollback. Catalog capacity is 4096 retained identities. All imported entries
become revision/digest/kind floors; no address is returned until a higher signed
revision is accepted. Even maximum revisions remain retained and cannot wrap.

Work artifacts live in a new sibling `.rds-migration-<random>/` with mode 0700:
`intent.json`, `migration.lock`, `source.redb`, and initially `staged/`.
The intent identifies the source and requested destination. The staging directory
contains `migration.pending`, which ordinary FileStore startup refuses. All floors
and format-3 metadata commit in one transaction; its anchor and initialized marker
are then synced. The importer reopens and validates the catalog while retaining
ownership, writes/syncs `receipt.json`, removes/syncs the pending marker and performs
an exclusive rename of the complete staging directory to the requested destination.
Both rename parents sync before the command reports success. An independently
created destination is never overwritten. Receipts bind source/destination database
identities, generations, original source/anchor BLAKE3 digests and imported counts.

The durable receipt describes **prepared conversion**, not successful live cutover.
A crash or sync error after rename can leave a complete destination even when the
command did not report success. Preserve all artifacts, compare the receipt with
the complete catalog and reconcile the interrupted operation before proceeding.
Before rename the destination is absent; an incomplete staging catalog retains
its refusal marker. After marker removal a retained staging catalog is complete,
but it is not the configured destination. Never remove markers to force startup.

There is no automatic retry, resume or cleanup. Failed attempts may retain a partial
copy and staging files, and successful attempts retain the audit copy and receipt.
Reserve up to **512 MiB plus small metadata** of additional space for the bounded
copy and destination database; file allocation and sync errors abort publication.
The copy may differ from original bytes after engine recovery: the receipt's source
digest always identifies the original, not repaired copy bytes. Removing audit
artifacts or reusing an interrupted target needs a separate explicit inspection.
Physical power-loss and filesystem qualification remain required; process-exit
tests do not establish device write-cache durability.

## Source formats and retained meaning

| Source | Evidence in repository history | Treatment |
|---|---|---|
| Format 2 | `0dea767`, `records-v2` / `metadata-v2`; positive revisions and versioned signed record/delete envelopes | Verify the complete bounded catalog and generation anchor, then preserve each revision/digest/kind as a retired format-3 floor. Require a newer publication before serving addresses. |
| Format 1 | `fdeabd9`, `records-v1` / `metadata-v1`; earlier timestamp ordering/signature contract | Separate authenticated cutover procedure. Do not reinterpret timestamps as publisher revisions or invent domain-separated signatures. |
| Per-key legacy JSON | Audit baseline `87e7aea`; deletion could remove replay history | Separate authenticated cutover/inventory. A directory of remaining files cannot reconstruct deleted history or determine current GDS membership. |
| Format 3 | Current catalog and boot-bound leases | Normal protected open/recovery, not migration. No format conversion should reset leases or lower floors. |

The format-2 conversion cannot reconstruct the original continuous-clock lease.
Imported addresses therefore start unavailable; retaining a wall-clock expiry
alone would rearm old signed content. A publisher with durable issuer history
can allocate its successor through the existing 410 path. Missing issuer history
or a 409 remains a repair condition, never permission to guess a counter from an
untrusted server response. Retired floors are local integrity state, not signed
transferable claims.

## Contract and remaining deployment boundary

1. Use the explicit Rust CLI operation with distinct source and new destination;
   no automatic conversion during normal server startup. Require stopped source
   ownership using the same protected directory and OS-lock rules as FileStore.
   Resolve all file access through held no-follow directory/file descriptors.
2. Require an initialized source anchor and database identity. Bound directory
   enumeration, database bytes, cells and identities before allocation/iteration.
   Refuse unknown tables/formats, malformed envelopes, mismatched signed keys,
   invalid revisions, count mismatches and rollback of the database generation.
3. Preserve source database and anchor bytes. The pinned redb builder's read-only
   entry point opens a path; the owned backend entry point may repair/write.
   Do not hand the original source path to either as an ownership shortcut.
   Use a bounded copy from the locked source descriptor into protected new work
   state, then allow engine recovery only on that copy. Account for the temporary
   copy in the migration disk budget; do not silently truncate on exhaustion.
4. Accept a coherent source generation or the existing narrowly defined one-ahead
   interrupted-commit case only after verifying the whole recovered catalog.
   Never accept an older repaired root. Bind a local migration receipt to the
   source database identity, source generation, source/anchor digests, destination
   identity and imported counts. Keep the receipt in a protected sibling audit
   location rather than weakening the active store's unknown-file refusal.
   This receipt does not replace the W4 external
   GDS anchor against rollback of the entire trusted directory.
5. Write all imported retired floors and metadata in a bounded destination
   transaction, then the durable anchor/initialized state. Keep a durable pending
   marker until destination validation and receipt synchronization finish. Normal
   FileStore open must refuse incomplete migration state. No HTTP success or
   readiness can make a partially imported catalog visible.
6. Reopen and validate the owned staging catalog before publishing the destination.
   A repeat against
   any existing destination must refuse rather than overwrite it; an interrupted
   attempt retains its source and identifiable destination for inspection.
   Removing owned temporary copies needs an explicit, bounded recovery rule.
7. Keep live configuration cutover separate from conversion. Verify membership,
   publisher issuer state, policy anchors and successful successor publications
   before accepting a deployment receipt. Preserve the old directory offline;
   do not serve both generations or roll back to an older catalog after new
   acknowledgments without an authenticated reconciliation procedure.

## Required fixtures and exit evidence

- Generate format-2 fixtures from the historical schema, including a record,
  deletion, expired content, maximum revision and an empty initialized catalog.
  Every imported record requires a successor; exact/lower replays stay refused.
- Verify old signatures before deriving floors. Forgery, wrong key, trailing
  bytes, weak/invalid identities, corrupt anchor, wrong database ID, unknown table,
  oversized input and catalog/count mismatch must leave the source unchanged.
- Exercise source owner contention and directory/database/anchor substitution,
  source/destination aliasing, existing destination and bounded temporary space.
- Inject interruption before/after copy, destination transaction, anchor, receipt,
  marker removal and parent-directory sync. Incomplete targets never serve an
  empty or partial replacement. Compare source bytes before and after all exits.
- Prove one-ahead recovery preserves the newer floor and behind-anchor recovery
  refuses. Whole-directory backup rollback remains externally anchored W4 work.
- Use real publisher/directory HTTP to prove post-conversion 410 → durable
  successor → exact retry after a lost reply → restart without revision loss.
- Qualify maximum-size conversion, both supported OSes and the documented
  deployment/rollback procedure before lifting the format migration hold.

The timestamp/legacy cutover is intentionally a separate task: it needs a signed
GDS membership and retirement decision, protocol/domain inventory and preservation
of available history. A format-2 importer is not sufficient evidence to approve
that older deployment path.
