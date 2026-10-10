# Checkpoint W6-TABBED-WORKSPACE — 20261010-064726

Verdict: **automated checkpoint PASS; later close-callback correction recorded separately**

Source: clean `ab341dffbf5c4ea2b72af66a36c94430c8064a19`.
1,136 passing test executions, zero failed, two explicit native ignores.

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

This checkpoint covers window-owned physical modifiers and the previously
qualified tab/clipboard/texture behavior. Native clipboard/window evidence and
physical/network/installed limits remain in the workspace reports. The later
close-with-reply fixture reproduced an extra paste during window exit; its
correction and exact native before/after evidence belong to the next source.
No new unsafe block, protocol tag or authentication policy was introduced.
