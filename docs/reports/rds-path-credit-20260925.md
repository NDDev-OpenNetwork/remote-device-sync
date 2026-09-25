# Owned path-credit retries — 2026-09-25

Scope: W3.6 bounded candidate allocation retry, base `b3b2c09`.
Linux synthetic evidence; no backend promotion, deployment or wave closure.

## Correction

TLS may finish before spare peer connection IDs or multipath credits arrive.
One-shot additional-path opens then reject a usable address and never try again.
A deterministic 200 ms one-way simulation reproduced that defect: the automatic
attempt was missing, while the same secondary listener validated when opened
manually after credits arrived. The fixture asserts typed temporary exhaustion
before waiting, rather than inferring it from a missing path.

The existing weak policy task now owns a bounded candidate queue. Temporary
credit errors back off under one 15-second lifetime; allocation/permanent errors
remove the entry. Limits are 41 addresses, eight due opens per iteration and
25 ms exponential backoff capped at 400 ms. Queued duplicates preserve their
budget; mapped IPv4 aliases share an entry. Withdrawals remove advertisement-only
entries while independent ticket addresses survive. Candidate snapshots after
subscription and QNT lag cover missed address updates, not lost path-validation
events. Paths remain Backup until Established; allocation is not validation.
See [contract](../path-selection.md).

## Evidence

Five queue unit cases cover cap/expiry, duplicate/backoff behavior, withdrawal
and independent sources, bounded work/permanent refusal, and mapped aliases.
Two delayed-link simulations check automatic later validation plus replacement
data, and last-handle drop during pending backoff with the endpoint retained.
The targeted library/path/lifecycle/simulation run passed 32 tests across four
targets, including existing weak ownership and validated-selection regressions.

Final formatting and default/X11/all-feature Clippy passed. Workspace: **351
tests across 62 targets** (one previously qualified migration-capacity
test ignored). All-feature network/agent/CLI/relay: **142 tests across
33 targets**. Isolated owned network: **32 tests across
4 targets**. The [machine-readable receipt](rds-path-credit-20260925-data.json)
records commands, durations and source hashes. No dependency, wire version,
unsafe code or runtime helper was added. `cargo-deny` is unavailable locally.

## Remaining work

Initial candidate family fairness, path-event reconciliation, complete metrics,
relay/child socket failure isolation, physical interface/NAT transitions and
service recovery remain required. The 15-second limit is per queue admission;
a later new advertisement can admit the address again. Global connection/churn
bounds and reconnect jitter are not established. Native macOS is unqualified.
No remediation wave is closed by this increment.
