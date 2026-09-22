# Checkpoint C6 — 20260922-102437

Verdict: **pending review**

## Automated checks
- fmt/clippy/test: PASS (default + transport-noq lanes)
- sync_e2e (10 tests): PASS
  - push byte-identical; pull byte-identical
  - identical content resend: 0 chunks on the wire (dest-hash dedup)
  - corrupt part deleted + refetched; torn journal meta rebuilt
  - kill mid-transfer: resume fetches only missing chunks
  - G6 repeated kill/resume loop: converges identical
  - path traversal / absolute / NUL rel_paths refused (proptest fuzz)
  - impaired lane (lossy ImpairingSocket, noq backend): completes
- agent e2e: one sync session per connection (slot released on end);
  sync unconfigured refused; Info advertises Sync iff --sync-dir: PASS
- bounds: manifest ≤512/batch, chunksets ≤4096/batch, Need bitmap
  ≤256K chunks, every wire frame ≤64KB MAX_MESSAGE_LEN

## Manual checklist (fill before merge)
- [x] All "not run" items above explained — every check ran
- [x] Reports committed: this file; C6 evidence is the sync_e2e suite
  (now 12 tests — adds symlink-escape refusal and forged-chunk-len
  rejection)
- [x] docs/ updated: architecture.md documents resolved-path
  confinement and the v3 uni demux
- [x] security/unsafe review done: review found sync confinement was
  lexical-only (symlinked components could escape the root — fixed by
  `resolve_under` on pull/journal/assemble plus `.rds-sync` namespace
  refusal), and `ChunkHdr.len` sized the receive buffer unchecked
  (now verified against the manifest first) — fixed in this wave
