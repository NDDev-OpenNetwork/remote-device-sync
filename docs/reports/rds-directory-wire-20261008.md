# Directory snapshot wire model — 2026-10-08

This wave adds a bounded wire model for the directory foundation. It does not
change the existing single-file route and does not advertise recursive sync.

`rds-sync::directory` now provides:

- a versioned snapshot header carrying the manifest root, entry count and
  metadata-byte budget;
- deterministic parts capped at 32 entries and 48 KiB serialized payload;
- receiver-side assembly that requires strictly increasing canonical paths,
  exact count and metadata totals, and a matching final BLAKE3 root;
- sender-side partification after canonical manifest verification;
- rejection of unsupported versions, empty/oversized parts, reordered paths,
  truncated snapshots and forged roots.

The assembler holds only the bounded manifest model and performs no filesystem
I/O. A future service route must still bind this model to signed admission,
opened-handle scanning, resumable journal operations, tombstones and conflict
policy. Until that work lands, single-file transfer remains the only advertised
sync capability.

Validation on the development host:

```text
cargo fmt --check                         PASS
cargo clippy -p rds-sync --all-targets -- -D warnings  PASS
cargo test -p rds-sync directory::wire    PASS (3 wire tests)
```
