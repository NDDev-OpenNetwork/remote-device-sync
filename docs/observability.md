# Observability contract and execution plan

Scope: W10.1/W10.2, implemented incrementally alongside correctness work.
This is not a closure of W10, a production deployment or a distributed tracing
implementation. See [current remediation state](remediation-progress.md).
The [2026-09-25 Linux receipt](reports/rds-observability-20260925.md) records
444 workspace and 239 expanded tests, the actual collector/backend pipeline,
and two 100-probe development ping runs with JSON telemetry enabled.

## Stack and ownership

Use **Vector + OpenObserve**, as requested by the owner. Both are replaceable
infrastructure outside the Rust product data path. RDS depends on neither
backend availability nor vendor SDKs. The shared Rust `rds-observe` crate uses
existing workspace libraries; additional direct development dependencies are
the already locked `reqwest` and `data-encoding`, for the opt-in local API test.
The collector sends standard HTTP JSON logs and Prometheus remote-write metrics.
OpenObserve provides queries, dashboards and scheduled alerts in one backend.
There is no evidence here that switching backends would improve this project's
requirements. Existing estate telemetry and notification destinations remain
the deployment owners' responsibility; GitHub remains the CI run-status source.

Upstream contracts checked on 2026-09-25:

- [Vector ingestion into OpenObserve](https://openobserve.ai/docs/ingestion/logs/vector/)
- [Prometheus remote write](https://openobserve.ai/docs/ingestion/metrics/prometheus/)
- [Vector buffering guarantees](https://vector.dev/docs/architecture/buffering-model/)
- [Vector secret backends](https://vector.dev/docs/reference/configuration/secrets/)
- [OpenObserve scheduled alerts](https://openobserve.ai/docs/user-guide/analytics/alerts/)
- [OpenObserve configuration](https://openobserve.ai/docs/administration/configuration/environment-variables/)

```text
Rust binaries → bounded stderr adapter → local log capture/rotation
                                               ↓
                               Vector schema/privacy projection
                                    ↓                  ↓
                            HTTP JSON logs    Prometheus remote write
                                    └──── OpenObserve ──┘
                                         queries / alerts
```

## Process logging

All five executables share the same initializer. `RDS_LOG_FORMAT=text` is the
default for local diagnostics; `json` enables the schema below. Invalid format
or `RUST_LOG` values fail before product initialization. CLI parsing happens
first so `--help` does not start a logging worker. Logs go to **stderr**;
stdout remains command output, including identities and benchmark receipts.
Legacy consumers that parsed stdout logs must switch to stderr.

`RUST_LOG` controls diagnostic verbosity (agent/server/relay default `info`,
CLI/bench `warn`). Typed operational events and numeric connection context
remain enabled even with `RUST_LOG=off`. JSON excludes free-form messages,
errors, arbitrary structured fields and Debug/Display formatting. A diagnostic
event retains static target/line/level for locating its source. Full text is
private local debugging output: do not export it as if it were redacted JSON.
The entrypoint returns an exit code instead of invoking Rust's synchronous
terminal error renderer after logging shutdown. JSON does not format the final
error or its source chain; text mode routes it through the same bounded output
adapter. A private remote error cannot reappear in JSON mode or inject
schema-shaped log lines. CLI parse errors
before initialization and help output still follow Clap's terminal behavior.
Collection projects only schema-1 records even when stderr contains other output.

Each JSON record contains:

| Field | Meaning |
| --- | --- |
| `schema_version` | `1`; incompatible versions fail closed in the collector |
| `timestamp_unix_us`, `uptime_us` | wall clock for search; monotonic elapsed process time for diagnosis |
| `service`, `version` | fixed executable role and Cargo version; not an exact build attestation |
| `run_id`, `sequence` | random 128-bit process ID and local event sequence; concurrent arrival order may differ |
| `session_id` | numeric agent connection ID, scoped to `run_id`; never a peer key or distributed trace ID |
| `level`, `target`, `line` | static source metadata |
| `event`, `operation`, `outcome`, `elapsed_us` | allowlisted operational fields; absent fields are null |
| `telemetry_*_total` | cumulative queue rejection, oversized-record and output I/O error counts |

No key, credential, address, user path, command, file content or remote output
is part of the export schema. A typed field with an unknown string value is
not copied. The collector independently reconstructs an allowed field map and
validates fixed-cardinality labels. Deployment supplies an opaque `node` label,
not a real hostname. Correlation IDs stay in logs, never metric labels.

Typed events cover process start/return, 15-second heartbeat, listener readiness,
agent allowlist admission/rejection, handshake failures/timeouts, connection
budget rejection and local connect/service-handler completion. Membership
admission does **not** mean a grant has authorized every service. A service
handler returning `ok` does **not** prove remote presentation, durable transfer
or even a successful service request: a handler can finish after replying with
a protocol error. Future service-specific success receipts must come from the
owning protocol. Cancelled observed operations report `cancelled`. A process
heartbeat means its main future is polled, not that remote access is healthy.
Abrupt termination, process abort and a stalled runtime need external liveness
checks; a final event cannot be guaranteed.

## Bounds, failure and shutdown

Records are bounded to 4096 bytes before allocation/queue insertion; oversize
records are discarded whole. The queue holds 1024 records (at most 4 MiB of
payload plus overhead). Producers use `try_send` and never wait for output
capacity or network I/O. One dedicated synchronous stderr adapter thread
isolates potentially blocked OS writes. This is an infrastructure adapter,
not a transport/service loop or a second async runtime. Using Tokio's blocking
pool for a permanently blocked stderr write would prevent runtime shutdown.

Shutdown seals producers, drains pending records and waits at most 500 ms for
the worker's completion notification. A stuck OS write cannot be safely
cancelled: the single worker is detached on timeout and reclaimed at process
exit. `Telemetry::shutdown` returns `drained` and final local counters; the
binaries currently make this best-effort shutdown and do not turn lost logs
into a product failure. There is no claim of lossless audit storage. Text-mode
Debug implementations can still do arbitrary CPU/allocation work; JSON does
not invoke them. Neither mode promises zero formatting cost.

The supplied Vector file source uses acknowledgments/checkpoints and a 512 MiB
disk buffer for logs. Full buffer stops file consumption. Product log rotation
and filesystem quotas remain necessary: rotating unread files can lose data,
and an exhausted filesystem can stop capture. Metrics use a bounded memory
buffer with drop-newest behavior. Retries can duplicate logs; use
`(run_id, sequence)` to deduplicate. The local shutdown receipt is not backend
delivery acknowledgment. Vector's disk buffer also has a documented crash
sync window; graceful restart evidence is not power-loss qualification.

Collector export admits a fixed set of buffer/error/uptime families. It rejects
per-file/per-endpoint series rather than merging distinct counters by erasing
their identity, and removes global hostname/PID metadata. Operation histograms
convert microseconds to seconds and use explicit buckets from 50 microseconds
through one hour. Incremental series expire from the collector cache after
ten idle minutes, so rare operations can restart their observational counter.
Its loopback-only `127.0.0.1:9598` scrape provides an independent local view during
backend trouble. Do not expose that listener through a public proxy. Product
transport counters remain in `rds-net::metrics`; the old directory-listener
metrics route is **not** the planned dedicated secured admin surface.

## Development qualification

Configs live in `ops/observability/`. `images.env` pins the tested upstream
Vector 0.58.0 and OpenObserve 1.0.4 manifest digests. Pin updates require rerunning
the pipeline regression. Compose is a disposable development fixture, not a
production service definition: it has an internal collection network, a
management bridge with an ephemeral loopback UI/API port, no Docker socket mount, disabled OpenObserve outbound
telemetry and no embedded credentials. The test generates a new private
directory, random password and unique project name, then removes only those
resources. It uses synthetic events and an internal webhook receiver; no human
notification is sent. The smoke overlay alone opts into OpenObserve's supported
loopback webhook setting and shares its network namespace with a fixture
receiver. It retains general private-address/cloud-metadata SSRF protection.

Pre-pull the exact images from `images.env` using Docker. Then, with one Cargo
process at a time:

```sh
cargo build --locked -p rds-observe --example fixture
cargo test --locked -p rds-observe --test pipeline -- --ignored --nocapture
```

The pipeline regression is opt-in because it starts local infrastructure. It
checks actual Rust JSON output, clean stdout, collector rejection/projection,
OpenObserve log search, controlled ok/error counters and histogram units through
remote write, alert minimum-volume/20-percent/loss predicates, scheduled delivery
to a local receiver, and log
recovery after a collector restart while the backend is unavailable. Default
workspace tests exercise redaction, output failure, queue saturation, record
bounds, filtering, cancellation, heartbeat and bounded shutdown without Docker.

For a persistent local development instance, prepare a private environment
file outside the repository with `RDS_OO_USER`, `RDS_OO_PASSWORD`, absolute
`RDS_OBSERVE_LOG_DIR` and `RDS_VECTOR_SECRET_DIR`. Use a strong development
password and provision the `authorization` file described below; never commit
that file or the environment. Capture selected JSON stderr files as `*.jsonl`
in the log directory and keep their rotation/retention bounded. Then run:

```sh
docker compose --project-name rds-observe-dev \
  --env-file ops/observability/images.env --env-file /private/rds-observe.env \
  -f ops/observability/compose.yaml up -d
docker compose --project-name rds-observe-dev \
  --env-file ops/observability/images.env --env-file /private/rds-observe.env \
  -f ops/observability/compose.yaml port openobserve 5080
```

`/private/rds-observe.env` is a placeholder for your actual private file. Open
the reported loopback address in a browser and log in with that development
identity. Use the same command prefix with `down` to stop it; omit `--volumes`
to preserve data. Production rollout must use its own reviewed configuration.
Only the automated smoke test uses `compose.smoke.yaml` and its receiver.

Vector config validation and its embedded projection tests can also be run
with `vector validate` and `vector test`. Vector 0.58 requires explicit
`--dangerously-allow-env-var-interpolation` for the non-secret, operator-owned
environment template. Credentials use the directory secret backend: provision
one private `authorization` file under `RDS_VECTOR_SECRETS`, containing the
standard `Basic ` prefix plus base64 of the exact `user:password` bytes, without
a trailing newline. Base64 is a reversible credential, not encryption; protect
the file as a secret. This avoids Vector 0.58's text-level substitution
interpreting quotes/backslashes in a raw password as configuration syntax.
Compose mounts `RDS_VECTOR_SECRET_DIR` read-only.
The integration password contains quotes/backslashes to check secret resolution
without raw config interpolation. Never load untrusted config or print the
resolved configuration. Production must use HTTPS with certificate/hostname verification
and a dedicated ingestion identity. The fixture root identity and plain HTTP
are restricted to the disposable internal network/loopback test.

## Queries, alerts and incident diagnosis

Search one process/session with exact `run_id` and numeric `session_id`. Compare
`operation_completed` outcomes and `elapsed_us` for `connect`; don't combine
cancelled attempts with failures. Check `telemetry_*_total` before interpreting
event rates. Counter gaps and backend duplicates make log-derived metrics
observational signals, not authoritative resource accounting or SLOs.

`alerts.json` contains disabled definitions for process failure, telemetry loss
and connect failure ratio. Bind them to an existing estate destination and
explicitly enable them only after qualification in that deployment. SQL
deduplicates attempt IDs before calculating the error ratio; the default
threshold is at least ten attempts and at least 20% failures in five minutes.
Loss alerts group by run so process resets do not masquerade as recovery.
They repeat after the silence interval while a run still reports loss.

For a failed connection, inspect startup outcome → discovery/policy local
diagnostics → connection outcome → allowlist admission → handler outcome →
transport snapshot and its coverage flags. Reproduce with the affected exact
build and a short private `RDS_LOG_FORMAT=text RUST_LOG=...` capture when typed
reason codes are insufficient. A raw debug capture is **not** a redacted support
bundle. Preserve it outside the public repository and apply local retention.

Backend/collector outage cannot reliably alert through the same backend. The
estate must provision an independent probe/watchdog and expected-instance
inventory; absent heartbeat alerts require knowing which daemon should be
running. A silent alert channel alone proves nothing.

## Remaining sequence and exit criteria

| Step | Work | Exit evidence |
| --- | --- | --- |
| O1 | Shared bounded schema, adapters, collector, basic queries/alerts | Rust and real pipeline regression receipts; this change |
| O2 | Dedicated authenticated/local-IPC admin surface for agent, relay and directory; source metrics for admission, policy freshness, task/queue counts and path coverage | No public/proxy bypass; controlled traffic reconciles snapshots; unknown/unavailable remains explicit |
| O3 | Stable reason codes and phase timing for discovery, grants, dialing, relay migration, SSH, decode/present and durable sync; negotiated operation IDs | Same operation traced across peers without credentials; cancellation/error/remote acknowledgment distinguished; loss/latency tests |
| O4 | Redacted bounded support bundle, exact build/features/policy digests and GDS status; saved dashboards | Secret-canary tests, bounded archive, actual failure localization; no raw logs by default |
| O5 | Private estate rollout using existing telemetry/alert channels; rotation, retention, quotas, TLS/roles, expected instances, independent liveness, collector/backend self-monitoring | Real host receipt, outage/recovery and notification delivery checks; no public credentials |
| O6 | Regression automation, latency/RSS/task overhead, restart/disk-full/power-loss and Linux/macOS qualification | Reproducible rds-bench reports and platform receipts; no W10 closure from a local fixture alone |

These steps accompany the existing W0–W10 plan. Native SSH/PTY, the desktop
viewer and platform capture backends, sync completion, transport recovery and
release qualification retain their own acceptance gates.
