# RDS remediation execution

Started: 2026-09-24. Plan: [remediation-plan.md](remediation-plan.md).
Audit baseline: `87e7aeabaad861b0d3c63f6641c91539be19a3e0`.
Audit/plan snapshot: `6772d4a`. Historical findings stay in the dated audit;
this file records implementation progress rather than rewriting that evidence.

## Current state

All waves remain open. W1.1/W1.2 passed the local Linux check matrix; the
other tasks remain planned unless listed below. No deployment or owned-backend
promotion has occurred. Native macOS checks still require their platform lane.

| Task | State | Evidence / remaining scope |
|---|---|---|
| W0.1 | Partial | R01 and R10 are regression tests in `rds-agent/tests/e2e.rs`; baseline failures were observed before the fix. R02–R09 and desktop body cancellation still need their owning fixes/tests. |
| W1.1 | Implemented; Linux checks passed | Denylist replacement retains its value without observers; atomic modification preserves concurrent revocations. Subscribe-before-check and initial watchdog snapshot check remove missed-update windows. Durable feed freshness remains W1.4. |
| W1.2 | Implemented; Linux checks passed | One authorization state owns admission, replay reservation and watchdog. ACK failure/cancellation closes the connection and releases the grant. Service admission checks live validity/revocation. Connection future teardown runs RAII cleanup. |

## Authorization change

`rds-agent::authz` owns Pending → Authorizing → Granted → Closed. Services
remain pending while an authorization response is being written. The watcher
is installed before that I/O; a 15-second response deadline bounds it.
Duplicate Authz cannot replace a pending or granted lease. Failed verification,
response write, interrupted commit and canceled admission close the connection.
Closing the connection cannot be undone by a late admission task.

The watchdog checks the current denylist immediately, then observes updates
and wall-clock expiry. Each service also checks policy directly so correctness
does not depend only on watchdog scheduling. Replay reservations and watchdogs
are released through owned drop guards, including cancellation of the
connection service future.

Regression evidence before implementation:

- `revocations_survive_without_watchers`: failed; initial revoked ID was lost.
- `concurrent_revocations_are_not_lost`: failed; fewer than 16 concurrent IDs survived.
- `failed_authz_reply_closes_connection_and_releases_grant`: failed; stopped
  reply retained the connection beyond the test's bounded observation window.

Additional coverage exercises simultaneous admission ownership, a snapshot
already present before watchdog startup, admission cancellation, immediate
service-scope revocation/expiry and reservation cleanup.

Local validation on 2026-09-24: `cargo fmt --check`; workspace clippy with
default features, `rds-desktop/x11`, and all features (all with `-D warnings`);
`cargo test --workspace`; `cargo test -p rds-agent --all-features` (5 unit and
15 integration tests). All passed. Native macOS execution is still pending.
`cargo-deny` is not installed locally. No wave checkpoint is claimed: these
changes do not close W1 or replace historical benchmark evidence.

The signed revocation feed still needs durable anti-rollback/freshness/offline
policy (W1.4); this patch does not close all of audit A02. Audience binding and
renewable leases remain W2.3. Long-lived service task ownership remains W2.5.

## Next sequence

Address W1.6/W1.7 partial-delta assembly and staging-file ownership, followed
by W1.8 filesystem confinement and W1.3 signed name trust. Each change retains
its own failing-before/passing-after evidence. No general CI or long soak is
made a blanket barrier to independent development; actual invariant failures
are repaired and platform evidence is recorded separately.
