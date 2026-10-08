# Directory scanner — 2026-10-08

The second W8 wave adds the confined scanner behind the public
`rds-sync::directory` model. It is a snapshot primitive; it does not advertise
or implement recursive transfer.

The scanner:

- opens the configured root once and traverses each child through an already
  held directory descriptor with `openat`/`fstatat` no-follow operations;
- sorts directory names by raw bytes for deterministic enumeration;
- streams regular-file content through the existing bounded FastCDC reader;
- records symlink targets with a fixed buffer and never follows them;
- rejects FIFOs, sockets, devices and unknown filesystem objects;
- bounds entries, path/target/metadata bytes and nesting depth;
- checks cancellation between entries and at file chunk boundaries;
- compares open-file and open-directory metadata before and after the scan and
  fails closed if an observed object changes.

Validation on the development host:

```text
cargo fmt --check                         PASS
cargo clippy -p rds-sync --all-targets -- -D warnings  PASS
cargo test -p rds-sync                    PASS (38 unit/integration tests)
```

The scanner is blocking filesystem work and must run on `spawn_blocking` from
async services. Recursive manifests still need a wire encoding, tombstones,
conflict policy, journal operations and end-to-end crash/reconcile tests before
the capability matrix can move beyond single-file sync.
