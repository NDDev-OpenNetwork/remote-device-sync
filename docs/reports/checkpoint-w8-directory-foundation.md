# Checkpoint W8-DIRECTORY-FOUNDATION — 20261008-181755

Verdict: **foundation checks passed; recursive service not qualified**

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- directory contracts: PASS
- release benchmark: bench-w8-directory-foundation.{json,md}; every sample verifies exact wire bytes
- topology: synthetic in-process; no remote recursive service or installed CPU claim
- NOT RUN: recursive apply/crash recovery, cross-platform metadata, two-way service; these remain open gates

## Reviewed checklist
- [x] All "not run" items above explained
- [x] Reports included in the signed evidence commit: benchmark JSON/Markdown, contract report and this file
- [x] docs/ updated for anything this wave changed
- [x] security/unsafe review done for new code paths
