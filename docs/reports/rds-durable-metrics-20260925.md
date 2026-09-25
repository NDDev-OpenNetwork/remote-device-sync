# Durable catalog and policy observations — 2026-09-25

Scope: W10 / O2 continuation on base `a0776ca1f70084e09770abed52fc270d0ebb9fb7`.
This is an implementation receipt, not a wave closure or deployment assertion.
The [contract](../observability.md#durable-catalog-and-policy-observations)
defines metric interpretation and the [machine receipt](rds-durable-metrics-20260925-data.json)
binds validation to changed source hashes.

## Implementation

Record owners publish aggregate stored-record/identity counts, capacity, storage
mode, health and the last acknowledged database generation. FileStore publishes
after both immediate database and generation-anchor commits; memory stores have
no durable generation. Expiry collection preserves replay floors. Reopen first
validates/reconciles the catalog, then publishes the recovered observation.

PolicyStore publishes authority epoch and independent registry, revocation and
name-cache revision/count/lease observations from the already verified payloads.
Publication adds no signature verification or catalog scans. Refused input and
idempotent retries leave the observation unchanged; rotations clear old streams.
The effective agent denylist now carries epoch/revision beside its IDs and lease
in the same watched value. Stale/canceled feeds cannot update its successor.

A weak metadata-only handle owns neither database/file locks nor product tasks.
The publisher holds a separate short mutex only while copying fixed-size fields;
readers use try-lock and release before formatting. Store operations and policy
commits expose unknown while running. An uncertain disk result exposes unhealthy
and suppresses inventory/revision/freshness in exported metrics. Custom record
stores can decline observation support; export never calls their `len()`.

`Reading::cached_now()` cannot initialize/read boot identity from the filesystem.
An uninitialized or failed cache makes the observation clock unknown. Remaining
lease duration is bounded by signed wall expiry and the original continuous
deadline, including suspend; invalid boot/backward clocks close freshness.
Policy freshness also observes the global committed wall floor. Integer seconds
round down, with freshness reported separately for subsecond validity.

No wire/persistence formats, dependency versions, default transport, cryptographic
primitives or collector/backend configuration change. RDS owns its state and
publication semantics; existing Rust libraries continue to supply QUIC/TLS,
signatures and redb transactions. Vector/OpenObserve remain replaceable consumers
of the existing authenticated numeric exposition.

## Regression evidence

- Separate publication-lock contention, in-progress operation and owner drop
  produce unknown without retaining or waiting on the store.
- FileStore observations track put, exact retry, quota refusal, deletion,
  bounded collection, retained identity capacity and reopen. A held database
  mutex does not block the observer; an admission callback sees unknown before
  commit.
- Three database/anchor transaction failure phases suppress unacknowledged
  state; successful reopen publishes the recovered generation. Five policy
  persistence failure phases report unhealthy and omit revision/freshness,
  then reconstruct the appropriate recovered revision.
- Policy tests cover invalid signature, stale revision, quota refusal, exact
  replay, both clock deadlines, reboot/backward readings, global wall floor,
  registry/name-cache counts, durable reopen and authority rotation.
- Effective agent observations cover revision/lease consistency, expiry,
  unavailable clocks, local authority, feed failure and obsolete-feed fencing.
- Real directory HTTP traffic exports exact inventory with no per-device labels;
  the existing stalled custom-backend lifecycle fixture stays unsupported rather
  than exporting a false zero. Actual server admin HTTP exposes durable catalog
  gauges while the public metrics route stays absent; authentication and joined
  shutdown fixtures remain active.

## Validation

The full Linux workspace passed: **472 passed, 2 ignored, 80 result targets**.
Workspace/all-target Clippy passed with warnings denied in default, X11 and
all-feature configurations. Workspace/shared admin fixture formatting passed.
The expanded `rds-discovery`, `rds-agent`, `rds-server` all-feature suite passed:
**211 passed, 1 ignored, 26 result targets**. It overlaps the workspace suite.
The ignored tests retain their existing opt-in qualification requirements.
All seven Linux commands in the machine receipt passed on the same source hashes,
with two Cargo build jobs and sequential compiler invocations for this task.
The final diff whitespace and local documentation links were also checked.

The attempted `aarch64-apple-darwin` all-target discovery check **did not pass**:
`ring`'s C build first selected the Linux `cc`, which rejected Apple flags. A
second attempt explicitly selecting installed Clang 18 still selected Linux
`stdint.h` and failed on missing `bits/libc-header-start.h`. No usable Apple SDK
is configured on this host. Both exit-101 results remain in the receipt; neither
attempt reached checking the discovery crate, so macOS compilation and native
execution remain unqualified. No compiler flags, dependency features or source
were weakened to produce a successful cross-check.

## Remaining qualification

No native macOS execution, physical suspend/power-loss, deployment or production
alert delivery is claimed. `cargo-deny` is not installed in this environment.
The real Vector/OpenObserve pipeline was qualified in the preceding
[admin receipt](rds-admin-metrics-20260925.md); it is not rerun here because its
configuration, renderer and ingestion path are unchanged. New names satisfy its
existing bounded-prefix projection; this is not a new live-backend receipt.

No benchmark was run for this increment and no connection/latency improvement or
observability-overhead bound is claimed. Concurrent scrape/service resource
qualification remains O6. Fine task/queue/upstream-relay coverage, phase/reason
correlation, diagnostics/dashboard artifacts and private rollout remain O2–O6.
SSH/PTY, native desktop capture/viewer, sync completion, transport recovery and
release gates remain open in the remediation plan.
