# Checkpoint W1-SYNC-SUPERVISION — 20261011-003337

Verdict: **pending review**

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- legacy refusal/FIN with receive half retained on Iroh and Noq: PASS
- terminal refusal projection on both wire profiles: PASS
- quiet control with paused time and abandoned queued assembly: PASS
- existing v1/v2 completion, cancellation, journal and recovery contracts: PASS
- local journal preparation regression benchmark: 100 samples/case
- physical cancellation, power loss and installed qualification: NOT RUN

## Manual checklist (fill before merge)
- [ ] All "not run" items above explained
- [ ] Reports committed: bench-*.json, bench-*.md, this file
- [ ] docs/ updated for anything this wave changed
- [ ] security/unsafe review done for new code paths
