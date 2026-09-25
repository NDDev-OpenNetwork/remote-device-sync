# Durable policy validation — 2026-09-24

Scope: remediation W1.4, completing the durable part of W1.3 and the policy
freshness part of A02/A05. Base implementation: `d0949c1`; independent desktop
measurement correction: `4de7b2e`. This receipt describes the containing policy
change, not a release or deployment. All inputs are synthetic fixture identities.

## Behavioral evidence

Before implementation, regressions accepted a signed revocation snapshot from
one hour in the future and a snapshot with a one-day lifetime. Both assertions
failed on the old API and pass with the new stamp/domain/lifetime contract.

| Suite | Evidence |
|---|---|
| `revocation_safety` | Future snapshots and excessive lifetime refused |
| `policy_state` | Same-second revisions; duplicate/equivocation/rollback; restart and new boot; wall/continuous steps; exact expiry; authority rotation continuity; global cross-name history; stale bootstrap; corruption, missing state, symlinks/hardlinks and single-writer ownership; wire/domain/size bounds |
| `policy::tests` | Injected write failure and abrupt child exit before write, after write, after file sync, after rename and after directory sync; no uncertain commit published; restart reads a complete old/new state and rejects older committed revisions |
| `managed_revocations` | Five real loopback QUIC cases pass on both iroh and explicitly selected noq: missing policy denies admission; valid snapshot opens; newer revocation closes active and new access; outage/replay cannot extend cached lease; same-boot restart preserves denylist; feed drop and disk-write failure close active access |
| Agent feed ownership unit | Concurrent owner refused; obsolete completion cannot publish after shutdown or invalidate successor |
| Directory/name integration | Directory restart ignores older bootstrap and refuses expired revocations on GET; separately constructed clients share durable cross-name revision history |
| `worker_budget` | Timed-out HTTP request retains its disk-worker permit until the worker completes; saturation responds 429 and recovers |

The full workspace initially exposed a desktop fixture race: a drain counted
65 arrivals while the producer remained active and mistook that for occupancy
above 64. The correction measures instantaneous occupancy and drains a bounded
batch, retaining ordering/freshness assertions. No queue cap or latency budget
was raised. The focused corrected test passed.

## Validation environment and limits

Local environment: Linux x86_64, rustc 1.98.1, cargo 1.98.1. Private command logs
are retained outside the public module. Final matrix results are recorded in
[remediation-progress.md](../remediation-progress.md).

No native macOS, physical suspend/power-loss, external GDS rollback anchor,
Cloudflare edge or real estate deployment was exercised. Process exit is not a
power-loss simulation. The new clock adapter's source references and supported
platform semantics are in [platforms.md](../platforms.md). The Rust code uses
existing locked `rustix`, `rand` and macOS `libc` dependencies, with one bounded
read-only platform FFI call; it adds no runtime subprocess or external daemon.

No latency/throughput qualification is claimed by these correctness tests.
W0/W3 measurements, native platform checks and all wave-close gates remain open.
The operational and wire migration contract is [policy-state.md](../policy-state.md).
