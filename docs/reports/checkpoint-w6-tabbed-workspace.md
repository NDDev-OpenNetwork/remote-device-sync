# Checkpoint W6-TABBED-WORKSPACE — 20261010-130947

Verdict: **implementation checkpoint PASS; native synthetic chrome acceptance PASS**

Code source: `a9515128f0fdfdba0b045b17d9a77bf9ff51db86`.
1,142 passing test executions, zero failed, two explicit native ignores.
The release benchmark observed the clean source tree. Native checks are scoped
in [the chrome report](rds-workspace-chrome-20261010.md).

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- desktop/CLI feature Clippy and library/launch contracts: PASS
- simultaneous real peer and monitor channels on both transports: PASS
- release tab-state churn: 100 samples each for 1/4/8 open tabs
- native UI, physical clipboard/input, GPU/memory soak and installed qualification: separate review required
- retained impaired control RTT failure: OPEN; no threshold change

## Manual checklist (fill before merge)
- [x] All "not run" items above explained
- [x] Reports included: bench-*.json, bench-*.md, this file
- [x] docs/ updated for anything this wave changed
- [x] security/unsafe review done for new code paths

Review: native macOS still-picture checks cover panel hiding/restoration,
ordinary/fullscreen geometry and two complete cycles over three displays from
two synthetic devices. Idle CPU image restoration does not re-admit frame
timing. The redraw self-loop was reproduced with the optional fixture reporter
and eliminated. Pointer ownership and DPI/resize geometry have regression
coverage. No unsafe code, dependency, credential authority or wire change.

Physical input/clipboard, long-duration GPU/resource soak, glass-to-glass
latency and installed fleet adoption remain separate. The reported intermittent
black specks were not reproduced; the established integration defects and the
idle-tab black screen have scoped evidence. The retained impaired control-reader
incident remains open despite this passing checkpoint.
