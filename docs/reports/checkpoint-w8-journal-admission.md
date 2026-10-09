# Checkpoint W8-JOURNAL-ADMISSION — 20261009-230142

Verdict: **PASS for bounded journal preparation; broader stability remains open**

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- rds-sync cancellation, confinement, recovery, directory and transfer checks: PASS
- local release journal benchmark: 100 samples/case; verified 4 MiB resume,
  pre-canceled admission, admission beside 8192 retained foreign names
- physical power loss, disk quotas, fair background GC, recursive apply: NOT RUN

## Manual checklist (fill before merge)
- [x] All "not run" items above explained in the source-bound journal report
- [x] Reports committed: bench-*.json, bench-*.md, this file
- [x] docs/ updated for anything this wave changed
- [x] security/unsafe review done for new code paths

Combined source `aeb8045`: 1,050 passing executions, zero failures, two ignored.
This gate does not close the separate destination-binding follow-up or the
retained main-branch control-latency failure. See
[the detailed review](rds-journal-admission-20261010.md).
