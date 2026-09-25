# Observability contract and execution plan

Scope: W10.1/W10.2, implemented incrementally alongside correctness work.
This is not a closure of W10, a production deployment or a distributed tracing
implementation. See [current remediation state](remediation-progress.md).
The [O1 2026-09-25 Linux receipt](reports/rds-observability-20260925.md) records
444 workspace and 239 expanded tests, the actual collector/backend pipeline,
and two 100-probe development ping runs with JSON telemetry enabled.

## Stack and ownership

The [native SSH client](ssh.md) adds fixed `ssh_connect` and `ssh_session`
operation names to both the Rust and Vector allowlists. Outcomes/timings contain
no host, account, command, key or terminal bytes. JSON SSH requires a separate
`rds --log-file <new-private-path>` sink, capped at 8 MiB; never export its raw
stdout/stderr, which could contain a forged log envelope from a remote program.
The collector reads only the dedicated telemetry file. The path is exclusive,
in a validated private directory; the launcher owns naming and retention.
A completed session may carry a nonzero remote exit code; it is still a
completed SSH exchange. Terminal CLI logging without a separate file pauses
behind an acknowledged output barrier while SSH owns the UI, then resumes
after termios/descriptor restoration. The existing bounded queue
and overflow counter apply; long interactive sessions can delay/drop CLI logs.
Agent source telemetry is independent and continues during terminal use.

The default [local session manager](local-sessions.md) adds authenticated aggregate
`rds_agent_local_manager_*` gauges for available snapshot, connected/pending
sessions and capacity. Its weak nonblocking observer contains no peer or session
labels; outgoing samplers use the existing agent network metrics registry.
These names fit the existing Vector admin allowlist. Local tests do not imply a
fresh collector/backend run or operational alert rollout for this increment.

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
transport counters remain in `rds-net::metrics`; they are now included in the
agent admin snapshot with their existing observation/coverage limits. The old
unauthenticated directory-listener metrics route has been removed.

## Authenticated admin metrics (O2)

Agent, standalone relay and composed server accept the same paired flags:
`--admin-addr 127.0.0.1:3342 --admin-token-file /private/rds-admin-token`.
The listener is **disabled by default**. Non-loopback and wildcard addresses
are refused. The token and bind are validated before endpoint identity/catalog
creation. Agent startup no longer waits for relay connectivity: local service
and admin supervision start after endpoint bind even with a disabled/unavailable
relay. Directory announcements update as transport addresses become available;
the printed startup ticket is only the address snapshot at that moment. Local
listener readiness does not promise remote reachability.

`:0` selects an ephemeral port for tests; private text diagnostics
report the bound address. JSON exports deliberately omit addresses.

Create a separate scrape credential with the Rust operator CLI:

```sh
rds admin-token --file /private/rds-admin-token
```

The parent directory must already exist and be private. This creates 32 random
bytes encoded as 64 lowercase hex characters, prints no secret, initializes no
endpoint and never replaces an existing file. An optional final LF is accepted
when loading a deployment-provisioned credential. The final entry must be a
regular file owned by the process's effective user, owner-readable, with no
execute/group/other permissions or extra hardlinks. Symlinks and special files
are refused; nonblocking open prevents a FIFO from parking startup. Ancestor
directories and the local OS identity are trusted. Unsupported platforms fail
closed. Rotate by provisioning a new credential and restarting the daemon and
collector; there is no implicit reload or fallback credential.

Only `GET /metrics HTTP/1.1` with a single `Authorization: Bearer <token>`
returns data. A vetted constant-time primitive compares the credential.
Forwarded/Proxy-Authorization headers never grant access; ordinary Host values
are not an authorization boundary. Body framing, duplicate authority/auth
headers, transfer encodings, upgrades and origins are refused. Already buffered
trailing bytes are rejected; closing the connection prevents a second request
even when TCP delivers it later. Every response forbids caching. No token,
request header or path is echoed or logged by the listener. The old
`/v1/metrics` route returns 404 even to a loopback reverse proxy: public and
admin exposure no longer share a listener. Directory counter names now use the
`_total` suffix; update old queries during migration. Writer-label series and
the old scrape-time record inventory are absent. A missing series must not be
interpreted as zero inventory or zero device activity.

Limits: 16 retained requests, 8 KiB headers, 32 header fields, two seconds for
one read/snapshot/write exchange, 256 numeric samples including admin health,
and 64 KiB response text. Excess connections close immediately. Metric source
callbacks are synchronous, bounded in-memory observations: no network/disk I/O
or long critical sections belong there. Admin request cancellation releases its
slot; explicit shutdown joins the owned request set, while Drop aborts it.
Configured admin runner failure is supervised by its daemon. A collector or
OpenObserve outage has no connection to that supervision and cannot stop RDS.

Source semantics:

- Agent: occupied connection slots and service tasks, configured budgets,
  grant/revocation state and the existing transport registry. Freshness is
  omitted when grants are not required; local revocation authority is an
  explicit separate flag. A busy grant table is unknown, never a zero. Epoch,
  revision and remaining lease describe the effective watched revocation value,
  published after acceptance, not an in-flight disk result.
- Directory: parsed-request/publication/rejection/expiry counters and retained
  request/worker/maintenance task gauges. Malformed HTTP requests have a separate
  counter. Completed-but-unreaped handles count
  against their task budgets. Busy or released groups report `*_known=0` and
  omit their gauges. Record inventory and durable policy observations are
  published by transaction owners; scrapes never lock/read the durable catalog.
  See the durable observation contract below.
- Owned relay: actual forwarded/dropped datagrams and payload bytes, admission,
  attached endpoints and bounded recent-flow history. Its table observations
  use try-locks with explicit unknown flags. `admission_rejected_total` counts
  capacity/drain refusals, not every authorization/handshake failure.
- The iroh relay has no comparable forwarding snapshot in this adapter:
  `rds_relay_metrics_available=0`; unsupported values are absent. Availability
  of an observation handle alone is not application readiness.

Observers retain counters or weak references, never a store/connection/task
owner across an await. These are individual concurrent observations, not a
transaction across every counter; existing path sampling can miss short-lived
paths/final increments. Names are fixed product metadata, values are unsigned
numbers, and only static direct/relay path labels are admitted. Stable writer
hash labels were removed because they still identify device activity.

For collection, load the optional `ops/observability/vector-admin.toml` fragment
alongside `vector.toml`. Supply the private `RDS_ADMIN_URL` (including `/metrics`),
`RDS_ADMIN_SERVICE` (`rds-agent`, `rds-relay` or `rds-server`) and an opaque
`RDS_OBSERVE_INSTANCE` unique within that node/service. Put the exact credential
in the collector secret directory as `admin_token`, without a trailing newline.
If collector and daemon
use different OS users, provision a separate private collector-owned copy;
do not weaken the daemon token file's permissions. The existing remote-write
sink includes the optional admin projection. Source metrics carry only opaque
node/instance/service and permitted path labels, never the scrape address.
The scrape source disables HTTP proxies explicitly, including inherited proxy
settings, so its bearer credential stays on the local connection.

The collector must share the daemon's loopback network namespace. Do not change
the bind to `0.0.0.0` to make a container bridge work, publish it through WARP/a
tunnel, or attach its credential to a public reverse proxy. Plain HTTP here is
restricted to local loopback; remote collection requires a separately designed
authenticated/TLS or local-IPC boundary. `RDS_VECTOR_LOCAL_METRICS_ADDR` can
choose another loopback exporter port when the default 9598 is occupied.
The disposable Linux qualification uses an unprivileged, capability-free Vector
container sharing host networking solely to reach the synthetic local listener.
Its ports and credentials are unique to the fixture.

References: [Vector's Prometheus scrape contract](https://vector.dev/docs/reference/configuration/sources/prometheus_scrape/),
[HTTP message framing](https://www.rfc-editor.org/rfc/rfc9112.html),
[constant-time equality](https://docs.rs/subtle/2.6.1/subtle/trait.ConstantTimeEq.html).


### Durable catalog and policy observations

`RecordStore::metrics()` is optional; custom backends without an observer report
`rds_directory_records_supported=0` and `records_known=0`, with no invented
inventory. Built-in stores provide weak, fixed-size metadata observations.
Directory initialization acquires these handles once. A scrape does not call
`len()`, verify signatures, scan rows, acquire a store/policy transaction lock or
read boot identity from disk. `Reading::cached_now()` samples the platform clocks
only if normal product work has already initialized the boot cache; otherwise
freshness is explicitly unknown.

Each owner copies metadata through a separate short publication mutex. Readers
use `try_lock`, copy the entire group and release it before formatting. No I/O,
validation or callbacks run with that mutex held. Record operations and policy
commits mark their observation unknown while work is in progress. A panic leaves
it unknown; detected storage uncertainty reports `healthy=0`. Public metrics omit
inventory/revision/freshness when unhealthy. Exporter handles retain no files,
database, file locks, tasks, signed payloads, device keys, names or grant IDs.

| Group | Observations and interpretation |
| --- | --- |
| `rds_directory_records_*` | `supported`, `known`, `healthy`, `durable`; `stored` counts record payloads awaiting retirement, not currently fresh/reachable peers; `identities` includes tombstones and retained floors; `capacity` is the identity limit; `generation` is present only for the durable database |
| `rds_directory_policy_*` | `configured`, `known`, `healthy`, `durable`, current authority `epoch`, and `clock_known` |
| `rds_directory_{registry,revocations,name_cache}_*` | `present`, committed `revision`, aggregate `entries`, `fresh`, `lease_remaining_seconds`; absent streams omit revision/count/freshness |
| `rds_agent_revocations_*` | Effective `snapshot_present`, `epoch`, `revision`, `local`; when grants are required, `clock_known`, `fresh` and managed `lease_remaining_seconds`. Local authority has no managed TTL. No lease means closed managed admission; an unavailable clock omits freshness for an existing lease |

Database metadata is published only after both database and anchor commits.
Exact retries do not advance generation; expiry collection can advance it even
when a request returns expiry. Reopen validates/reconciles storage first, then
publishes the recovered generation. Unacknowledged writes may become visible
on successful recovery, as specified in [record state](record-state.md).
In-memory inventory carries no durable generation. Neither expiry collection
nor deletion releases remembered identity slots.

Policy revision, count and lease are one observation. Signature/revision/quota
refusal leaves the previous observation; identical retries never extend its
lease. Rotation clears old streams. Freshness checks the committed policy wall
floor, boot identity, wall expiry and the original suspend-inclusive deadline.
Seconds are rounded down: zero remaining seconds can still be fresh for less
than one second, so use the separate freshness gauge. Revisions remain visible
for expired healthy policy. Failed persistence omits them until successful
reopen. Agent failure retains the last effective revision for diagnosis while
clearing its live lease; a late obsolete feed cannot publish a successor's state.

These are coherent groups, not a cross-process/distributed transaction or a new
authority for authorization/recovery. Prometheus-compatible consumers may round
large integer revisions to floating-point precision: use the authenticated
product state, never telemetry, to decide exact revision ordering. Labels remain
bounded; the existing Vector admin projection accepts these fixed metric names.
See [the implementation receipt](reports/rds-durable-metrics-20260925.md).

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
remote write, authenticated admin scrapes through the merged Vector config,
exact source counters/labels, disabled scrape proxying, alert
minimum-volume/20-percent/loss predicates, scheduled delivery to a local
receiver, and log recovery after a collector restart while the backend is unavailable. Default
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
| O2 | Authenticated loopback admin listener and aggregate source metrics implemented; durable policy/record observations now implemented; remaining: finer task/queue coverage and upstream-relay adapter | Proxy rejection, real daemon authentication/shutdown, known traffic and collector receipts; unknown/unavailable remains explicit |
| O3 | Stable reason codes and phase timing for discovery, grants, dialing, relay migration, SSH, decode/present and durable sync; negotiated operation IDs | Same operation traced across peers without credentials; cancellation/error/remote acknowledgment distinguished; loss/latency tests |
| O4 | Redacted bounded support bundle, exact build/features/policy digests and GDS status; saved dashboards | Secret-canary tests, bounded archive, actual failure localization; no raw logs by default |
| O5 | Private estate rollout using existing telemetry/alert channels; rotation, retention, quotas, TLS/roles, expected instances, independent liveness, collector/backend self-monitoring | Real host receipt, outage/recovery and notification delivery checks; no public credentials |
| O6 | Regression automation, latency/RSS/task overhead, restart/disk-full/power-loss and Linux/macOS qualification | Reproducible rds-bench reports and platform receipts; no W10 closure from a local fixture alone |

### Next reviewable increments

1. Continue O2 coverage: durable policy/catalog observations now have commit,
   failure, expiry and reopen regressions. Add remaining task/queue coverage and
   adapt the locked iroh-relay 1.2.0 public `Server::metrics().server` counters
   through weak handles. Its `server` feature already enables metrics; no second
   upstream HTTP metrics listener is needed. Keep upstream frame/byte semantics
   distinct from owned-relay datagram/payload counters, and prove actual relayed
   traffic plus owner drop before changing availability. Preserve no disk/store
   locks during scrapes and explicit unsupported/busy/expired state. Production
   inventory/lease dashboards and alert delivery remain O4/O5.
2. Add O3 phase/reason contracts in the owning protocol crates, beginning with
   discovery → policy → connect → service admission. Keep fixed reason enums
   and separate local completion from peer acknowledgment. Acceptance: paired
   success, cancellation, timeout, rejection and connection-loss fixtures with
   bounded labels and no credentials/addresses in exported records.
3. Build O4 diagnostics from typed snapshots and exact build/config metadata.
   Bound collection time/archive size, redact secrets by construction and add
   secret-canary tests. Save dashboards against the tested schema. Raw private
   debug logs must require a separate explicit operator choice.
4. Qualify O5 in the private estate: expected daemon inventory, OS-user secret
   provisioning/rotation, TLS ingestion, retention/quotas and existing alert
   destinations. Prove missing-daemon and backend/collector outage detection
   independently of the failed component; record delivered notifications.
5. Run O6 resource and platform campaigns: concurrent scrape saturation during
   SSH/desktop/sync, startup/shutdown churn, RSS/FD/task plateaus, release-build
   latency distributions and native macOS. Report topology and feature profile;
   same-host ping alone cannot establish observability overhead or WAN latency.

These steps accompany the existing W0–W10 plan. Native SSH/PTY, the desktop
viewer and platform capture backends, sync completion, transport recovery and
release qualification retain their own acceptance gates.
