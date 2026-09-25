# RDS destination-bound grant and lease increment — 2026-09-25

Baseline: `beb3a7a578a782f31e51791b7ec8e50584eafb7f`. Scope: **W2.3 partial**,
local IPC v3 and W10 authorization observations. No wave-close or release-ready
claim. The [machine-readable receipt](rds-grant-leases-20260925-data.json)
contains 226 source/configuration hashes and private-log digests.
Raw logs stay outside the public module; fixtures use synthetic identities.

## Result and compatibility

[Grant v2](../grant-leases.md) adds protocol-domain signatures, exact bounded
payload parsing, strict intervals, a positive lease revision and the serving
device audience. Stable session IDs preserve revocation/replay ownership across
renewal. The issuer supplies a fresh cryptographically random 128-bit nonce for
each new authorization and retains it only for renewal.

One connection owns one replay slot/watchdog. Same-scope signed renewal extends
the live connection while retaining existing TCP streams; an exact retry never
resets its continuous-clock deadline. Wall rollback, suspend-aware deadline,
boot mismatch, stale policy, revocation and old-lease expiry remain fail-closed.
The two-slot fixture holds the only service-body slot while renewing through
reserved control capacity. Scope changes require revocation and a new session.

The remote ACK/commit/FIN order and local reply/publication/EOF barrier serialize
successive transactions. Cancellation with an uncertain result closes the pinned
connection instead of replaying a remote operation. Explicit `session renew`
updates the manager's credential digest without changing selection. Fixed
`grant_authorize`/`grant_renew` observations expose outcome and elapsed time;
no token contents enter the exported schema.

This breaks old grants and payload-hash revocation IDs. Reissue grants and
coordinate issuer/agent/client migration, including local IPC v3. Remote ALPN
and existing service framing remain unchanged; old peers reject the appended
renewal request without downgrade. No new dependency or unsafe implementation
was needed. No installed process was updated by this source change.

## Regressions and intermediate findings

Before implementation, tests for a raw, non-domain-separated signature and an
inverted signed validity interval both failed on the baseline. They now pass.
Further coverage rejects wrong destinations on iroh and noq, malformed versions,
trailing/oversized payloads, revision rollback, changed scope/identity and expired
renewals. An independent connection remains usable after rejected renewals.

The live fixture verifies bytes before/after the original expiry, an exact retry,
concurrent renewed-token replay rejection and closure after revoking the original
ID. Injected clocks verify subsecond expiry, suspend progression, rollback and
boot mismatch. Pending renewal keeps the original expiry/cancellation guard.
Managed fixtures cover repeated renewal, old/new credential fingerprints,
pinned selection, ongoing TCP traffic and original-ID revocation. CLI tests
cover explicit renewal arguments and cancellation after ACK without FIN.

Intermediate runs found and corrected remote FIN publication ordering, a reused
fixture nonce, an omitted local-revocation fixture setup and a mistaken telemetry
test index. Dead-socket recovery now waits for observed connection refusal in
its fixture before asserting recovery. Final checks also corrected a redundant
path qualification; one incomplete matrix was restarted after review found a
test expecting the library diagnostic instead of the CLI diagnostic. These runs
are not reported as final passes.

The first expanded workspace run also reproduced a pre-existing relay-process
fixture race: successful child exit was treated as immediate client-side QUIC
closure. The fixture now waits at most two seconds for actual tunnel closure,
then still requires the checked Drain notice. It does not equate Drain with
unavailability during grace. The targeted standalone/composed relay tests and
the full final matrix pass with this correction; relay product code is unchanged.

## Final local verification

Linux x86_64, Rust/Cargo 1.98.1. This run started one Cargo command at a time
with two build jobs. Total command times include debug compilation/linking and
are **not** connection-latency or resource acceptance measurements.

| Command | Result | Seconds |
|---|---|---:|
| `cargo fmt --check` | pass | 2.777 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | pass | 5.904 |
| `cargo clippy --locked --workspace --all-targets --features rds-desktop/x11 -- -D warnings` | pass | 3.522 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass | 3.715 |
| `cargo test --locked --workspace` | 529 passed; 3 ignored (91 targets) | 197.187 |
| `cargo test --locked --workspace --all-features` | 578 passed; 3 ignored (91 targets) | 207.281 |
| `cargo test --locked --workspace --all-features --test ssh openssh_interop_key_agent_exec_and_os_pty -- --ignored --exact` | 1 passed; 0 ignored (1 target) | 1.490 |
| `cargo-deny --all-features check` | pass | 3.568 |

Default and expanded counts overlap; they are not a sum of unique tests. Both
runs ignore the opt-in OpenSSH, capacity migration and full observability
pipeline cases. OpenSSH passes separately above. All 226 source/configuration
hashes remain unchanged across the final sequence.

Vector 0.58.0 at the recorded image digest passed `validate --no-environment`
and **nine projection tests**, including both grant operations. Tests ran in a
read-only network-disabled container with synthetic settings/credentials. The
first fixture invocation omitted the required environment-interpolation opt-in;
it failed before tests and was corrected to match the existing deployment/test
configuration. See [Vector's environment contract](https://vector.dev/docs/reference/environment_variables/).
Sink health, OpenObserve ingestion, real alert delivery and the full pipeline
were not rerun here.

## Remaining work

Tenant/policy revision binding and read/write/view/control/account resource
scopes remain W2.3. The lease revision implemented here is not an estate policy
revision. Automatic issuer APIs, refresh scheduling and reconciliation remain
W4.2; this increment requires an explicitly provided signed renewal.

GDS SSH host/account lifecycle, Rust account broker/reattachment, viewer/sync
manager APIs, native macOS, real suspend/clock discipline, physical WAN/NAT/relay
and simultaneous SSH/video/sync soak remain required. There is no complete
checkpoint or new rds-bench result. Keep all broader W0–W10 and O2–O6 gates open.
