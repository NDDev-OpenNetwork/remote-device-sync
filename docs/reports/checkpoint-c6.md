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
- [ ] All "not run" items above explained
- [ ] Reports committed: bench-*.json, bench-*.md, this file
- [ ] docs/ updated for anything this wave changed
- [ ] security/unsafe review done for new code paths
