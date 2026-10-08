# Directory foundation contract review — 2026-10-08

Source implementation: signed `1cbeaac`; the deterministic desktop fixture
repair is signed `f1a79c7`. This wave follows PR132–135. It qualifies local scan,
snapshot encoding and dry-run planning only; recursive service/apply is open.

## Correctness repairs

- Nested `.rds-sync` journals made a tree produced by the existing single-file
  receiver unscannable. Exclude the private namespace at every depth before
  enumeration retention; compare opened directory aliases by inode identity.
- Reserve entries and path bytes globally during enumeration, before retaining
  names; add narrower `ScanLimits`, cancellation and depth ceilings.
- Hash regular-file content through a 64 KiB buffer without building unused
  FastCDC chunk lists. Scans detect observed changes but are not atomic snapshots.
- Failed snapshot parts do not advance assembler counters. Finish verifies the
  received canonical order without re-sorting it.
- Add `project_destination` and `check_inputs`: structural plan validity alone
  cannot prove entry preconditions or authorize arbitrary operations.
- Directory deletion/replacement conflicts with descendant edits/additions on
  the other side, including descendants absent from the common base.

The ten new contract regressions exercise real nested journal output, aggregate
budgets, special files, invalid path names, root symlinks, in-read mutation,
byte-boundary splitting, exact borrowed/owned postcard layout, invalid-part
recovery, forged plans, Keep semantics and forty seeded convergence fixtures.
Linux also tests arbitrary invalid UTF-8 filename bytes; APFS cannot create that
fixture, so a direct path-parser regression covers it on macOS.

## Verification and retained failure

`cargo fmt --check`, workspace clippy, workspace tests, directory regressions,
`cargo deny check` and `scripts/checkpoint.sh w8-directory-foundation` passed on
macOS arm64. Workspace ignored rows are explicit: external OpenSSH interop
needs an unlocked test account, and the 4096-identity migration campaign is a
separate capacity qualification. Linux/x11 and both platform CI remain separate
PR evidence. No native recursive serving qualification is claimed.

The first checkpoint at `1cbeaac` failed in the pre-existing desktop fixture
`stale_and_foreign_frame_routes_never_reach_the_session_inbox`: the demux sent
`Stopped(0)` before a forged payload write, the server task panicked, then the
client timed out waiting for real frames. This was a fixture error, not a basis
for increasing timeouts. The repair waits for one deterministic early refusal,
accepts only typed `WriteError::Stopped(0)` for disposable foreign streams and
retains the twenty-real-frame session assertion. The focused test passed and
the complete checkpoint retry at `f1a79c7` passed. Stream cancellation semantics
were checked against [RFC 9000 §3.5](https://www.rfc-editor.org/rfc/rfc9000.html#section-3.5).

## Measurement

The [machine JSON](bench-w8-directory-foundation.json) and
[rendered table](bench-w8-directory-foundation.md) record ten alternating-order
samples per size, release build, exact source/toolchain, clean source state and
payload digests. The reference retains the old clone-and-serialize splitting
algorithm with the current shared part validator; it is not a separately
installed old binary. This makes the comparison about splitting/encoding, not
an installed CPU or network claim.

| Entries | Reference median | Borrowed median |
|---:|---:|---:|
| 1,024 | 6.315 ms | 1.240 ms |
| 8,192 | 44.264 ms | 9.014 ms |
| 32,768 | 171.551 ms | 34.574 ms |

Every measured sample checked identical part count, serialized byte count and
framed payload digest. The iterator eliminates manifest cloning and speculative
payload allocations, sizes each entry at most twice and serializes only emitted
parts. No hardware-independent performance threshold or installed CPU reduction
is inferred.

[The checkpoint](checkpoint-w8-directory-foundation.md) and append-only receipts
bind the evidence. Destructive apply/crash recovery, persisted tombstones,
metadata/alias policy, watcher repair and remote recursive admission remain
unqualified as defined in [directory-sync.md](../directory-sync.md).
