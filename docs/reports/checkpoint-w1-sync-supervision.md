# Checkpoint W1-SYNC-SUPERVISION — 20261010-022020

Verdict: **PASS for local sync supervision; overall platform qualification held**

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- legacy refusal/FIN with receive half retained on Iroh and Noq: PASS
- terminal refusal projection on both wire profiles: PASS
- quiet control with paused time and abandoned queued assembly: PASS
- existing v1/v2 completion, cancellation, journal and recovery contracts: PASS
- local journal preparation regression benchmark: 100 samples/case
- physical cancellation, power loss and installed qualification: NOT RUN

## Manual checklist (fill before merge)
- [x] All "not run" items above explained in the source-bound review
- [x] Reports committed: bench-*.json, bench-*.md, this file
- [x] docs/ updated for anything this wave changed
- [x] security/unsafe review done for new code paths

Clean source `89db733`: 1,072 passing executions, zero failures, two ignored.
Four journal benchmark cases each passed 100 samples. No wire, unsafe,
runtime dependency, data-I/O deadline or absolute session-budget change.
Cancellation remains cooperative; rename already in progress may commit.

The macOS desktop-feature CI lane separately failed the retained control RTT
gate at p95 411 ms across 601 probes (limit 400 ms). Its default workspace and
sync regressions passed. This local receipt does not override that failure or
qualify installed/native behavior. See the
[detailed review](rds-sync-terminal-supervision-20261010.md).
