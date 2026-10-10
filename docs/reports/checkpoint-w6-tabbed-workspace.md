# Checkpoint W6-TABBED-WORKSPACE — 20261010-051706

Verdict: **automated checkpoint PASS; native/installed acceptance remains open**

Source: clean `6008b424464f225e1bb2f6cd6e5cbb0fa72f0362`.
1,135 passing test executions, zero failed, two explicit native ignores.
Receipt chain: 19 records, verified intact.

## Automated checks
- fmt/workspace clippy/workspace tests: PASS
- desktop/CLI feature Clippy and library/launch contracts: PASS
- simultaneous real peer and monitor channels on both transports: PASS
- release tab-state churn: 100 samples each for 1/4/8 open tabs
- native UI, physical clipboard/input, GPU/memory soak and installed qualification: separate review required
- retained impaired control RTT failure: OPEN; no threshold change

## Manual checklist (fill before merge)
- [x] All "not run" items above explained
- [x] Reports committed: bench-*.json, bench-*.md, this file
- [x] docs/ updated for anything this wave changed
- [x] security/unsafe review done for new code paths

Review retains the previous private/native boundaries and historical failed
RTT cohorts. Source `6008b42` includes the clipboard handoff, with 178 passing
feature-library tests and a successful macOS production build. Native delayed
Copy/switch/Paste counters confirmed transfer ownership, but the subsequent
plain-Tab interaction exposed local-widget interception. Its later correction
and native revalidation are separate from this successful earlier checkpoint.
Neither source qualification nor a synthetic peer establishes installed fleet,
physical capture/input, full native clipboard or long-duration resource acceptance.
