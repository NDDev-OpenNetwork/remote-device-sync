# RDS directional service scopes — 2026-09-26

Baseline: `a3cdbd036b54d4cd2d4c1d17fd3ca59a38f5288c`. Partial W2.3/W4.4,
W2.5 ownership and W6.4 ACK semantics. No wave-close or release-readiness claim.
The [machine-readable receipt](rds-service-scopes-20260926-data.json) binds
228 source/configuration files and private verification-log digests.
Fixtures contain synthetic identities and content; raw logs remain private.

## Result

The [grant contract](../grant-leases.md) now distinguishes `SyncRead` from
`SyncWrite`, and `DesktopView` from its `DesktopControl` input modifier. Legacy
`Sync` and `Desktop` retain full access. Permissions are additive; combining a
narrow capability with a legacy broad one does not restrict the latter.

Sync direction is checked before path parsing or filesystem access. Refusals
carry fixed permission errors, preserve files and leave the connection usable.
An RAII slot spans both hello acknowledgment and the transfer, so errors and
cancellation release ownership; rejected concurrent acquisitions do not release
the current owner.

View-only sessions never invoke the input sink. Authorized input uses one lazy,
bounded blocking worker and reuses its backend. A successful `InputAck` requires
a successful sink result. Failed, unavailable and forbidden input receives no
success ACK; heartbeat remains a separate response. Cancellation discards queued
work and releases the sink after any current native call returns. Already running
native calls cannot be undone. ACK proves backend acceptance, not application
response or physical input-to-visible latency.

New service tags 6–9 are append-only. Grant v2, IPC v3 and service framing remain
unchanged. Old decoders reject grants containing new tags without a broad-scope
fallback. AgentInfo retains coarse service advertisements. Renewal cannot add,
remove or reorder scope; revoke and authorize a fresh session to change it.
No dependency, unsafe block, external product executable or installed process
change is introduced.

## Regression coverage

- All 64 combinations of broad/fine sync and desktop capabilities are verified;
  every single-capability scope change is refused by renewal. Existing tags are
  byte-stable and an old decoder refuses new tags.
- Real iroh/noq agents enforce read-only/write-only operations, including fixed
  refusal before malformed/existing/missing path handling; authorized transfers
  verify destination bytes. Canceled sync can be reopened on the same connection.
  A desktop control modifier alone cannot open desktop or sync.
- Real desktop transports use an explicit synthetic input sink. View-only invokes
  it zero times; failed injection never emits an ACK and heartbeat still works.
  Successful input acknowledges an actual sink call, not a headless no-op path.
- A single-thread Tokio test holds one blocking injection, fills the one-job
  queue, rejects overflow, cancels the worker and verifies queued input never
  executes and the sink is released. A separate sync guard test covers repeated
  admission refusal, early error return and task cancellation.

The final sequence checks unchanged source/configuration bytes. No
baseline-negative execution is claimed for this increment.

## Final local verification

Linux x86_64, Rust/Cargo 1.98.1; one Cargo command at a time with two build jobs.
Command durations include compilation and are not product performance metrics.

| Command | Result | Seconds |
|---|---|---:|
| `cargo fmt --check` | pass | 1.005 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | pass | 24.040 |
| `cargo clippy --locked --workspace --all-targets --features rds-desktop/x11 -- -D warnings` | pass | 24.033 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass | 26.365 |
| `cargo test --locked --workspace` | 535 passed; 3 ignored (92 targets) | 283.879 |
| `cargo test --locked --workspace --all-features` | 585 passed; 3 ignored (92 targets) | 335.211 |
| `cargo test --locked --workspace --all-features --test ssh openssh_interop_key_agent_exec_and_os_pty -- --ignored --exact` | 1 passed; 0 ignored (1 target) | 1.398 |
| `cargo-deny --all-features check` | pass | 2.817 |

Default and all-feature counts overlap; they are not distinct-test totals.
The default/full-feature runs ignore the opt-in OpenSSH, full-capacity migration
and complete observability pipeline tests. OpenSSH passes separately above.
Vector configuration is unchanged; no new ingestion/alert-delivery claim is made.

## Remaining work

Whole-root sync permissions do not implement per-path ACLs. Tenant/policy binding,
SSH account authorization, native display/seat/focus isolation and consent remain
open. The X11 screen/focus/key mapping and held-key lifecycle are W6.4 work; a
synthetic sink does not qualify physical injection. Stuck native calls need
platform qualification. Rendering, native Linux/macOS capture/input paths,
automatic GDS issuance/renewal, managed viewer/sync, physical network/failover,
suspend and long mixed-load/resource acceptance remain in the remediation plan.
No installed binary, authority policy, live collector or deployment was changed.
