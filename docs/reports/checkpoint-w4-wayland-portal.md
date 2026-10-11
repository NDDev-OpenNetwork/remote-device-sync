# Checkpoint W4-WAYLAND-PORTAL — 20261011-005114

Verdict: **pending native qualification**

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- strict portal desktop/agent feature Clippy: PASS
- real EI socket pair: two monitor regions, shared key holds, pause invalidation and silent peer timeout: PASS
- private token rotation, display numbering, exclusive state and unsafe aliases: PASS
- mapped BGRA/BGRx bounds and region geometry: PASS
- agent admission library with portal build: PASS
- 100 shared loopback handshakes: regression measurement only, not native Wayland latency
- attended capture/decode, permission revoke/restore and physical input: separate native report required

## Manual checklist (fill before merge)
- [ ] All "not run" items above explained
- [ ] Reports committed: bench-*.json, bench-*.md, this file
- [ ] docs/ updated for anything this wave changed
- [ ] security/unsafe review done for new code paths
