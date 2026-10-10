# Checkpoint W6-TABBED-WORKSPACE — 20261010-182826

Verdict: **documentation reconciliation checkpoint PASS**

Reviewed source: `51bf87057c6286496797529c24477905d630c8b6`.
1,143 passing test executions, zero failed and two explicit qualification ignores.
The benchmark observed the clean source before generated reports changed it.
The reconciliation is documented in [the review](rds-documentation-finalization-20261010.md).

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

Review: production/build/dependency/wire files are unchanged. This wave corrects
current contracts and retires superseded planning instructions; it does not
introduce unsafe code, widen authorization or promote unavailable capabilities.
The first run passed all test stages, then failed on a stale CMake compiler
cache dropping the local install prefix. Its original failure remains retained;
a fresh scoped Opus build tree restored the specified flags and prefix, and
the same complete gate passed. No system permissions or dependency pins changed.

Previous native chrome evidence remains source-equivalent and historical; no
new native UI, physical input/clipboard, GPU soak, installation or network
qualification is claimed from this documentation-only checkpoint. The original
intermittent toolbar specks and retained Noq control-reader incident remain open.

Ignored checks are explicit: `openssh_interop_key_agent_exec_and_os_pty` requires
installed OpenSSH tools and an unlocked isolated test account;
`full_capacity_migration_preserves_every_floor` is the separate 4,096-identity
signed migration capacity qualification. They are not both native-UI checks.
The later reviewed receipt corrects that classification without rewriting the
earlier hash-chain entry or converting either skip into a pass.
