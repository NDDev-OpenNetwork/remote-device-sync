# Directory manifest model — 2026-10-08

The first W8 wave adds the deterministic directory snapshot model without
pretending that recursive synchronization is already a product capability.

Implemented in `rds-sync::directory`:

- bounded entry count and symlink-target size;
- canonical UTF-8 relative paths using the existing traversal and journal
  namespace checks;
- explicit file, directory and symlink kinds;
- content identity for files and raw, never-followed symlink targets;
- stable sorting and a snapshot root independent of enumeration order;
- canonical re-verification before diffing;
- deterministic added/removed/modified/unchanged classification and exact
  identity-based rename candidates.

The module does not walk the filesystem, follow links, mutate files or alter
the existing single-file wire protocol. A future scanner must use the existing
directory-handle/no-follow layer, then feed this model. Recursive transfer,
tombstones, conflicts, metadata policy, watch/reconcile and journal GC remain
separate gates.
