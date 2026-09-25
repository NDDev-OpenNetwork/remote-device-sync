# Directory capacity qualification — 2026-09-24

Scope: W1.5 file-store saturation after HTTP commit `3d4ad08`. This introduces
`rds-bench directory-capacity`; it does not deploy a directory or close a wave.
All identities and addresses are synthetic. The benchmark never dials them.

## Method

The production 4096-identity and 256 MiB database bounds are exported for the
harness; their values and storage algorithms are unchanged. A run requires a
new private state directory and preserves it on every exit. Invalid odd/out-of-
range round counts are refused before state creation. Existing paths are refused
before the store is opened. The CLI guard check confirmed that a prior benchmark
anchor was unchanged after refusal, and invalid rounds created no directory.

The two-round scenario performs 16,384 individually durable signed mutations: 4096
large admissions, 4096 small renewals, 4096 large renewals, 2048 deletions and
2048 large reactivations, plus any expiry-collection transactions. Large records
use 32 IPv6 candidates, eight 512-byte
raw relay origins and all six service kinds. Zero-padded ports fill the raw URL
limit without using oversized DNS labels; no network availability is inferred.

A 4097th identity must be refused before admission accounting after fill, each
renewal, deletion/collection and reactivation. Existing identities must still
renew. Four orderly reopens validate the whole catalog, followed by comparison
against every expected signed record or deletion. Collection does not free
retained identity slots. Timing covers each storage call, including signature
verification and its durability commits, but excludes signing and file-stat
sampling. Reopen samples include catalog validation. File length and allocated
blocks are sampled after every completed measured operation.

## Run provenance

The first default-dev attempt filled all 4096 slots, reaching 36,704,256 logical
database bytes. It was deliberately stopped after 325.89 seconds to replace an
undersized overall harness deadline before the longer churn/reopen phases.
Its partial database and `capacity-1.log` are preserved privately. It is **not**
a completed qualification and no success JSON was generated for it.

The complete run uses the default dev profile with these explicit cryptographic
dependency optimizations, keeping all validation and persistence code paths:

```sh
cargo --config 'profile.dev.package.curve25519-dalek.opt-level=3' \
  --config 'profile.dev.package.ed25519-dalek.opt-level=3' build -p rds-bench
target/debug/rds-bench directory-capacity --state-dir <new-private-directory> \
  --rounds 2 --json <private-report.json> --md <private-report.md>
```

Linux x86_64, Rust/Cargo 1.98.1, shared development VM. This mixed dev/optimized-
crypto profile is a capacity check, not a production release latency baseline.
JSON/Markdown outputs contain counters and timings, not private state paths or
endpoint identities. Source-base SHA in the generated report identifies the
worktree parent; the harness changes and this receipt accompany its source.
The measured executable's SHA-256 is
`7713dbbb57281de31e88675367945be22805e5492d563b865230f958447d04fa`.

## Result and limits

The complete run passed in **652.70 seconds**. The
[machine-readable JSON](rds-capacity-20260924-data.json) and
[generated tables](rds-capacity-20260924-data.md) retain all six phase reports.

- **16,384 / 16,384** signed mutations and **4 / 4** orderly reopens passed;
  every reopened catalog matched its expected records/deletions.
- All five attempts to add a 4097th identity were refused before the admission
  callback. The intervening known-identity renewals/reactivations succeeded.
- Maximum sampled file length was **36,704,256 bytes (35.004 MiB)**; maximum
  allocated blocks represented **33,824,768 bytes (32.258 MiB)**. Logical length
  remained at its filled size; the allocated high-water mark increased by 32 KiB
  across renewals. No compaction or larger capacity limit was introduced.
- The large signed payload was **4805 bytes**. This fills the configured field
  counts/raw URL sizes, not the independent 8192-byte envelope ceiling.
- Renewal storage calls had p50 **27.44 / 25.97 ms** and p95 **56.67 / 59.42 ms**
  for shrink/expand respectively in this mixed profile.
- The four full-catalog reopens ranged from **1.54 to 15.98 seconds**. This is
  observable startup/recovery cost, not a per-request dial delay. Four samples
  do not establish a stable production percentile. Release-build startup and
  sustained renewals need profiling before a reconnect/availability target is
  accepted; verification must remain complete while optimizing redundant work.

Validation passed: default and optimized-crypto harness builds; CLI guard checks;
formatting; default/X11/all-feature workspace Clippy with warnings denied; and
the final `cargo test --workspace` run, **245 tests across 49 targets**.
Private `capacity-checks` logs retain the final matrix. The preceding HTTP
receipt separately records the 59-test all-feature network/agent/relay lane;
this harness step changes no transport implementation. `cargo-deny` is not
installed. Explicit migration remains required before format-3 deployment.
Native macOS, sustained production renewal load, concurrency/HTTP admission at
this scale, physical filesystem exhaustion, forced power loss and long churn
soaks remain separate qualification. No constant was raised to make a run pass.
