# Record expiry retention — 2026-09-24

Scope: the W1.5 expiry/clock-floor and memory-capacity step after publisher
commit `0dea767`. No deployment or wave closure. All identities, clocks,
addresses and temporary state in tests are synthetic.

## Change

Store each accepted mutation with its original suspend-inclusive lease. A
process restart preserves that deadline; an OS reboot requires a new signed
revision. Runtime backward wall time and time below the committed floor refuse
operations without discarding history. When a read observes expiry, commit its
retirement and raised clock floor before returning the expiry result.

Retirement removes signed content and retains a compact revision/digest/kind
floor as trusted local state. It is not a publisher-signed transferable proof.
The floor never expires automatically. Embedded memory and disk stores share
this ordering/expiry decision and have bounded identity counts. Capacity
refusal for a new identity does not forbid renewing an existing one.

The service owns one blocking collector, processing at most 64 identities per
pass on a one-second schedule. It has separate capacity from request workers,
does not overlap jobs, and stops scheduling on drop. A started transaction may
finish. Reclaimed database pages may be reused; the physical file need not
shrink. Collection metrics report retirements and failures.

HTTP 410 causes the publisher to commit one locally allocated successor on the
next poll. A lost successor reply then retries the same signed bytes. HTTP 409
remains fatal; no remote counter is trusted. There is no immediate server-reboot
push notification to an already acknowledged publisher.

## Regression coverage

- Eleven record-expiry unit cases cover same-boot restart, sleep-like continuous
  clock advances, forward wall expiry and backward time, OS boot changes,
  retained delete floors, a 65-identity two-batch sweep, strict postcard decoding,
  concurrent collection/renewal, and injected/abrupt failures around database
  and anchor commits. One of those cases is the crash-child entry point.
- Three memory cases cover matching clock behavior, bounded rotating collection,
  identity saturation, preserved replay protection and admitted renewal.
- Two service cases verify collection without lookup traffic and one owned
  maintenance job independent of the request-worker budget, including drop
  while the collector is blocked.
- A fourth publisher lifecycle case uses real HTTP, a persistent issuer, a 410,
  a deliberately lost successor reply and reopening the issuer for exact retry.
- Existing database partial-write/sync failures, atomic anchor boundaries,
  process-crash, replay, wire and policy regressions continue to apply to the
  shared transaction implementation.

Initial new integration fixtures did not compile because a condition-variable
guard was discarded and the network test lacked its direct `serde_json` dev
dependency. Both fixtures were corrected; failed logs are retained. No
production dependency/version or external runtime helper was added.

## Validation

Linux x86_64, Rust/Cargo 1.98.1:

- `cargo fmt --check`: passed.
- Default, X11 and all-feature workspace Clippy with warnings denied: passed.
- `cargo test --workspace`: **221 passed across 47 targets**.
- `cargo test -p rds-net -p rds-agent -p rds-relay --all-features`:
  **59 passed across 18 targets**, including the owned relay.
- Targeted checks: 38 discovery unit tests, two service-collection tests and four
  publisher-lifecycle tests passed.

No thresholds or protocol validation were weakened. Private evidence preserves
initial compile failures and the final matrix separately under `expiry-checks`.

## Remaining qualification

Format 3 refuses prior per-key JSON and format-1/format-2 databases. Explicit
migration, enrollment boundaries and write fairness remain W1.5 work; deployment
stays on its existing format until migration is ready. Identity slots cannot
be recycled simply by expiry. File-capacity renewal reservation is not yet
qualified. Whole-directory rollback needs the external GDS anchor.

Native macOS, physical power loss, real suspend and production latency have not
been exercised. Synthetic boot/time injection tests the state machine, not the
physical OS clock adapter. `cargo-deny` is not installed. No wave checkpoint is
claimed; required measurement and platform receipts remain open.
