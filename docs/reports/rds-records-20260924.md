# Record transaction validation — 2026-09-24

Scope: W1.5 storage foundation, based on `c633dab`. No release, deployment,
complete W1.5 acceptance or latency qualification. All identities are synthetic.

The three initial R02 regressions failed against the previous stores: replaying
a deleted record in memory, replaying after disk restart, and treating corrupt
stored JSON as absence. The replacement keeps tombstones and refuses corrupt
history. The corruption case now targets the database format explicitly.

Eight integration tests cover memory/disk delete replay, restart, corrupt and
missing state, database-only rollback, legacy-directory preservation, exclusive
ownership, symlinks/hardlinks and concurrent mutation ordering. Six unit tests
(including the child-process entry point) cover three database/anchor failure
boundaries, five atomic-anchor failure boundaries, abrupt process exits,
partial database writes, sync failure and backend growth/overflow bounds.
Readers never observe an uncertain commit; reopening recovers a complete
allowed generation or refuses the state. Process exit is not physical power loss.

Linux x86_64 with Rust/Cargo 1.98.1:

- Formatting and default, X11 and all-feature workspace Clippy passed with
  warnings denied.
- The default parallel `cargo test --workspace` run failed the existing
  desktop `soak_60fps` latency gate: 1031 frames over 20 seconds, p95 176 ms,
  p99 1447 ms. Its limits remain 150/250 ms; no production code, test limit or
  fixture was changed to hide this failure.
- One isolated diagnostic rerun passed: 1197 frames, p95 10 ms, p99 16 ms.
  The difference is consistent with concurrent load, but does not establish a
  root cause or qualify production latency.
- The complete sequential-test diagnostic run, `cargo test --workspace --
  --test-threads=1`, passed **184 tests across 44 targets**. It does not replace
  the recorded failure of the default command. W0/W6 test isolation and latency
  qualification remain open.

Private logs remain outside this public module. `cargo-deny` is not installed;
native macOS and physical disk/power failures were not exercised. Dependency
rationale and the migration hold are in [record-state.md](../record-state.md).
No wave-close gate was invoked because the wave remains open.
