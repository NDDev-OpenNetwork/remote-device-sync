# Directory enrollment and admission — 2026-09-24

Scope: the W1.5 enrollment and write-fairness step after expiry commit `928dae7`.
No deployment or wave closure. Fixtures use synthetic identities and local state.

## Reproduced before implementation

Both new cases failed against `928dae7`:

- `default_directory_refuses_unenrolled_publishers`: an unconfigured directory
  accepted a self-signed endpoint publication rather than denying enrollment.
- `refused_writer_cannot_spend_another_devices_renewal_budget`: a writer's
  per-key refusals spent the global pre-verification counter; a different
  remembered device then received rate limiting instead of renewing.

The private `admission-before.log` preserves these failures. Both pass after
the change. The previous test of a shared unauthenticated PUT/DELETE limiter was
replaced by an authenticated per-identity PUT/DELETE quota check because the
old global budget is the failure being removed, not an invariant to preserve.

## Change and bounds

Default service configuration denies endpoint record access. A static allowlist
of at most 4096 distinct nonweak keys is provisioned using repeated
`--directory-allow`; it is independent of name proofs, relay allowlists and
agent authorization. Removing membership hides retained records but does not
erase floors or close existing endpoint sessions. Synthetic fixtures opt into
open enrollment explicitly; the production CLI has no open-enrollment switch.

Signature/key/lifetime/order validation and the quota callback occur before
commit under the same storage owner. Exact retries, stale/conflicting revisions
and invalid signatures do not charge quota. Concurrent copies charge once.
The callback can refuse without changing a record or poisoning the store.
Raw store APIs intentionally remain primitives without service enrollment.

Each identity is capped before charging shared write capacity. Known identities
also retain one protected new mutation in a fixed 60-second window; new identity
admissions and extra writes use separate budgets. Policy registry and revocation
roles each have a separate budget charged only after validating a new revision.
Refused attempts do not debit identity or shared capacity. Store I/O failures
after admission do not refund the attempt.

The standard TTL/3 announcer with TTL >= 180 seconds fits protected renewal;
the default TTL is 300. Shorter lifetimes/extra changes need burst capacity.
Accounting resets on process restart, not per HTTP connection. Shared CPU,
worker, network and disk saturation remain possible: the guarantee is write
budget separation, not unlimited throughput or arbitrary-flood availability.
See [record-state.md](../record-state.md) for exact limits and trust boundaries.

## Additional evidence

- Seven real HTTP cases exercise default deny, quota starvation, duplicate/stale
  and forged messages, admission/extra-write saturation, membership removal,
  concurrent identical writes and independent policy roles.
- Four limiter/enrollment unit cases cover all 4096 protected accounting slots,
  bounded slot reuse/window reset, refusal without debit and weak/duplicate/
  oversized enrollment configuration. The 4096-slot case is accounting logic,
  not a filesystem/network throughput benchmark.
- Two transaction integration cases run against both memory and disk stores:
  callback refusal keeps the old mutation; delete/reactivation preserves known
  classification; concurrent exact copies invoke admission once.
- Existing policy persistence/crash, replay, record expiry/GC, HTTP, TLS,
  publisher and owned-relay tests remain part of the check matrix.

The first targeted Clippy run caught unnecessary module qualifications in
updated fixtures. They were fixed without suppressing warnings. No dependency
or external runtime helper was added.

## Validation and remaining work

Linux x86_64, Rust/Cargo 1.98.1:

- Formatting and default/X11/all-feature workspace Clippy with warnings denied:
  passed.
- `cargo test --workspace`: **234 passed across 48 targets**.
- `cargo test -p rds-net -p rds-agent -p rds-relay --all-features`:
  **59 passed across 18 targets**, including owned relay discovery/connection.
- `cargo run -p rds-server -- --help`: built and ran with `--directory-allow`.

Private `admission-checks` logs retain the final matrix separately from initial
regression and fixture-lint failures. Native macOS, physical failures and
production load remain unqualified. `cargo-deny` is not
installed. This change does not close W1.5: strict HTTP framing, explicit format
migration and database-file saturation/renewal qualification remain open.
GDS live membership reconciliation and authenticated retirement are W4.
