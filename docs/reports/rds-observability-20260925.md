# Process telemetry and collector qualification — 2026-09-25

Scope: W10.1/W10.2 foundation (O1), with W0 measurement evidence.
Source base: `428c718d6b47b78b546acf27722287e3d401d7db` plus the changed source
hashes in the [machine receipt](rds-observability-20260925-data.json).
Linux x86_64, debug builds, shared development host. No wave closes.

## Implemented contract

All five executables use the Rust `rds-observe` leaf. Typed lifecycle and local
operation events, random process identity, numeric session context and output
health counters share one initializer. JSON never formats arbitrary messages,
errors or Debug/Display fields; text remains private diagnostic output. CLI
stdout stays parseable. Final errors return an exit code through the bounded
adapter instead of a second synchronous terminal error renderer.

Records are limited to 4096 bytes and 1024 queue entries. Producers never wait
for output capacity. A dedicated synchronous stderr adapter keeps blocked OS
writes outside Tokio runtime shutdown. Closing seals admission atomically,
drains accepted records and waits at most 500 ms; stuck output can lose records.
The [contract](../observability.md) describes these limits and the O2–O6 work.

Pinned Vector 0.58.0 and OpenObserve 1.0.4 images form an isolated development
pipeline. Vector rebuilds the permitted JSON projection, exports observed
operation counters/histograms via Prometheus remote write, and keeps bounded
collector health labels. Logs use a 512 MiB disk buffer; metrics use a bounded
memory buffer. Private secret files carry a pre-encoded Basic authorization
header, preserving passwords with quotes/backslashes without raw TOML insertion.
No new external crate version, product-side backend SDK or backend dependency
is introduced. Vector/OpenObserve are replaceable infrastructure.

Three disabled alert definitions cover process errors, telemetry loss and at
least 20% connect failures with at least ten attempts in five minutes. Duplicate
log IDs are removed before the ratio is calculated. Deployment owners must
bind existing private destinations and independently monitor backend/liveness.

## Final validation

All eleven sequential commands in the machine receipt succeeded with two Cargo
build jobs and one Cargo invocation at a time:

- Workspace formatting and separate shared-fixture formatting.
- Workspace/all-target Clippy with warnings denied: default, X11, all features.
- Workspace: **444 passed, 2 ignored, 77 result targets**.
- Expanded net/relay/agent/CLI/server all-feature suite: **239 passed,
  0 ignored, 45 result targets**. These overlap the workspace suite.
- Explicit fixture build and ignored pipeline test: **1 passed**; actual Vector
  config validation and **5 embedded projection tests** also succeeded.
- Two development `rds-bench` ping runs, 100 probes each, with JSON telemetry.

The workspace includes 12 observer unit tests and the CLI stdout/error privacy
regression. Coverage includes secret canaries, never invoking arbitrary JSON
formatters, operational events under `RUST_LOG=off`, heartbeat virtual time,
success/error/cancellation, whole-record limits, 10,000-attempt saturation,
output/flush failures, blocked destructor/shutdown, and a concurrent admission
race repeated 32 times with four producers. Existing real relay/server process
fixtures were updated to read diagnostic stderr while preserving stdout checks.

The real pipeline queries the backend for exact safe records and controlled
ok/error counters, histogram counts and seconds-valued sums. It checks the
minimum-volume/20% error and loss alert predicates, then observes a scheduled
notification at an isolated local receiver. During backend downtime the fixture
emits seven new events; all seven unique IDs are recovered after collector and
backend restart. Source checkpoint replay can contribute to this recovery;
this is not an isolated disk-buffer or physical power-loss durability test.
The temporary project, volumes and secret directory were removed after testing.
Cached upstream images remain available for later qualification.

## Development measurements

| Backend | Probes | Ping p50 | Ping p95 | Ping p99 | JSON records |
| --- | ---: | ---: | ---: | ---: | ---: |
| iroh | 100 | 4.388 ms | 9.076 ms | 12.716 ms | 117 |
| Noq | 100 | 3.509 ms | 7.933 ms | 9.887 ms | 112 |

Both runs used `RDS_LOG_FORMAT=json RUST_LOG=info`; every observed local loss,
oversize and write-error counter was zero. Raw benchmark reports are embedded
unchanged in the machine receipt. Their `git=428c718` value is the base commit;
the receipt's file hashes identify the tested uncommitted implementation.
These are same-host direct-path probes over one established connection with
five warmups excluded. The harness prefers loopback but retains endpoint
defaults; this is not an enforced network-isolation qualification. Compilation
time in the command receipt is not ping latency. No connection setup, WAN/NAT,
release throughput, logging overhead A/B result or improvement claim follows.

## Limits and next work

Secured source/admin metrics, stable protocol reason/phase codes, cross-peer
operation IDs, exact build attestation, bounded redacted support bundles and
saved dashboards remain O2–O4. Private deployment, TLS/roles, rotation/quotas,
retention, expected-instance inventory and independently verified notifications
remain O5. Long-run overhead/resource/failure campaigns and native macOS remain
O6. Log-derived metrics are best-effort observational signals; handler completion
does not establish remote presentation or a durable transfer acknowledgment.

Native macOS, remote CI, actual remote devices and production rollout were not
run. `cargo-deny` is unavailable locally. This local work does not close native
SSH/PTY, desktop capture/presentation, sync, transport recovery or release gates.
