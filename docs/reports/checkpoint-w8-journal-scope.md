# Checkpoint W8-JOURNAL-SCOPE — 20261009-231739

Verdict: **PASS for destination-bound journal recovery**

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- destination isolation, legacy attribution, torn metadata, confinement and recovery: PASS
- grant-scoped real stream regression on Iroh and Noq: PASS
- local release benchmark: 100 samples/case, including equal-content destinations
- physical power loss, disk quotas, principal isolation and installed qualification: NOT RUN

## Manual checklist (fill before merge)
- [x] All "not run" items above explained in the source-bound review
- [x] Reports committed: bench-*.json, bench-*.md, this file
- [x] docs/ updated for anything this wave changed
- [x] security/unsafe review done for new code paths

Source `7cb2096`: 1,062 passing executions, zero failures, two ignored.
No wire, dependency, unsafe or receive-lock change. Legacy attribution,
destination binding and cleanup were reviewed against path-scoped access.
See [the detailed review](rds-journal-destination-scope-20261010.md) for evidence,
compatibility and the separate unresolved control-latency gate.
