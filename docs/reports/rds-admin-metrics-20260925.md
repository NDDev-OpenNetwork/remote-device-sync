# Authenticated source metrics and local readiness — 2026-09-25

Scope: W10.1/W10.2, O2 admin/source increment, plus W2.6 agent local startup.
Source base: `8aefccddc384c1b8884532fd6583b7f401436cb0` plus the changed input
hashes in the [machine receipt](rds-admin-metrics-20260925-data.json).
Linux x86_64, debug builds, shared development host. No wave closes.

## Changes and observed failures

A dedicated Rust admin listener serves agent, relay and composed server. It is
disabled by default and requires an explicit loopback address plus a private
bearer token file. The Rust CLI creates a new random credential without printing
it, replacing a file or initializing endpoint identity. Configuration/token/bind
preflight occurs before product identity/catalog creation.

The directory's old public-listener `/v1/metrics` route is removed. A regression
forwarding requests through a real owned loopback TCP proxy failed on the base
with HTTP 200 and now receives 404. Stable per-writer hash labels were removed;
they still disclose device activity. Aggregate numeric snapshots contain no
keys, addresses, file paths or peer labels.

The initial default workspace run exposed a separate startup defect: agent
unconditionally awaited iroh's relay-only `online()` predicate even with
`--no-relay`. The new process fixture missed its 20-second readiness deadline.
Agent now starts local service/admin supervision after endpoint bind. A real
agent with a deliberately nonresponsive local relay accepts an allowed direct
QUIC ping, exposes the accepted-connection count, and joins SIGTERM shutdown.
Announcements follow subsequent address changes; a printed startup ticket is
only that moment's snapshot. Local readiness does not prove WAN reachability.

Transport snapshot inspection found a blocking selected-path mutex read. The
new contention test failed before correction. Export now uses a try-lock,
reporting selection unknown without waiting or clearing the stored observation;
other counters remain available. RTT/cwnd zeros require the accompanying
`selected_path_known` flag. Existing sampled-counter limitations remain.

The first post-fix workspace run also timed out in the existing validated-path
replacement fixture while awaiting Available status. The identical executable
then passed 10 isolated and 40 paired repetitions; the single observed timeout
has no uniquely proven cause. Inspection found a missing application-policy
precondition: engine validation can finish before policy consumes Established
and its initial candidate queue. The fixture now waits for policy to observe
the replacement before closing the primary. Post-fault selection and datagram
deadlines are unchanged. This is a fixture-ordering correction, not evidence of
a production failover fix or absence of scheduling sensitivity.

## Boundary and ownership

Only authenticated `GET /metrics HTTP/1.1` invokes source callbacks. Duplicate
Host/auth/framing, bodies, transfer encodings, upgrades and origins are rejected;
proxy headers never authorize. Responses do not reflect request data and forbid
caching. There is one response per connection, with no keep-alive or redirect.
The contract bounds 16 retained requests, 8 KiB/32-field headers, a two-second
exchange deadline, 256 samples and 64 KiB exposition. Trusted in-process source
callbacks must remain bounded; an async deadline cannot preempt arbitrary
synchronous provider code.

Unix token loading checks the opened file: no-follow/nonblocking open, current
owner, regular type, single link, owner readability and no execute/group/other
permissions. Credentials use a vetted constant-time comparison and zeroizing
storage. Ancestor directories/local OS identity are trusted; unsupported
platforms fail closed. Rotation requires a restart.

Source adapters observe agent admission/grants/revocation freshness and transport
counters, directory request/publication/rejection/GC/task budgets, and owned-relay
forwarding/admission/history. They retain weak I/O references or counters alone.
Busy/expired observations are explicit; the upstream relay adapter reports
unavailable instead of manufacturing forwarding zeroes. Explicit shutdown joins
admin requests; canceled waiters retain the result; Drop aborts owned work.
Daemon supervision handles unexpected admin runner completion. Collector/backend
failure cannot stop product service.

New direct dependency edges (`httparse`, `subtle`, `zeroize`, Unix `rustix`,
optional CLI `clap`) use versions already present in the lockfile: bounded HTTP
parsing, credential handling, safe OS calls and one shared flag contract. No new
external package/version, product helper process or Vector/OpenObserve SDK is
introduced. The [contract](../observability.md) documents coverage and migration.

An additional Apple Silicon `cargo check` reproduced a test portability error:
`rustix::fs::mknodat` is not exposed on Apple. Target-specific fixture modules now
keep Linux FIFO nonblocking coverage and use a Unix socket entry on other Unix
platforms. Shared permission/link/content tests are preserved. The runtime code
is unchanged by this split; the receipt identifies the earlier test-file hash
and the subsequent final workspace, observer Clippy and Darwin checks. A cross
check is not native macOS execution or complete workspace qualification there.

## Real collector qualification

The optional Vector admin fragment uses directory-backed secrets and rebuilds
opaque node/instance/service plus direct/relay labels before the existing remote
write sink. Source HTTP proxies are explicitly disabled. The fixture supplies
an owned proxy with an empty bypass list and proves that no connection reaches
it while authenticated scrapes reach the backend.

An unprivileged, capability-free, read-only Vector container shares the fixture
listener's host network namespace. The first attempt exited without retained
logs; retaining the container revealed that a 512 MiB configured disk buffer
could not fit the fixture's 64 MiB tmpfs. State now resides in an owned private
disk directory and exited-container diagnostics are retained until cleanup.
Production buffer limits were not reduced to make the fixture pass.

The final pipeline checks the real Rust listener, merged Vector config, exact
counter values (42 publications, 123/456 direct/relay bytes), unavailable relay
coverage (0), and the backend label allowlist. It also reruns O1 log projection,
controlled outcome/histogram values, alert predicates and scheduled delivery to
an isolated local receiver. Seven unique records recover after collector/backend
restart; source checkpoint replay can contribute. This is not a power-loss
or isolated disk-buffer durability qualification. No human alert is sent.

## Final validation

All fifteen sequential commands in the receipt succeeded, with two Cargo
build jobs and one compiler invocation at a time:

- Workspace and shared-fixture formatting; final workspace diff whitespace check.
- Workspace/all-target Clippy in default, X11 and all-feature modes, warnings
  denied; focused observer all-target/all-feature Clippy after fixture portability.
- Final workspace: **466 passed, 2 ignored, 80 result targets**.
- Expanded net/relay/agent/CLI/server suite: **251 passed, 0 ignored,
  48 result targets**. It overlaps the workspace suite.
- Observer all-target/all-feature **Apple Silicon cross-check passed**.
  Native macOS execution and whole-workspace Mac qualification remain pending.
- Real pipeline: **1 passed**, including Vector validation and **5 embedded
  projection tests**, authenticated scrape/proxy checks, alerts and restart.
- Two 100-probe development ping measurements with JSON telemetry.

Coverage includes 20 observer unit tests, three admin tests per daemon plus the
additional iroh outage/direct-ping process case, source counters under known
traffic, bounded slow-client saturation, cancellation/drop/shutdown, malformed
HTTP, token links/permissions/special files and CLI creation without identity or
secret output. The source-contention regression passed after its fix. The
corrected two-test path fixture passed 40 paired repetitions. These repetitions
are not additional distinct functional tests. Owned fixture containers, volumes
and secret directories were cleaned; cached image/build artifacts remain.

## Development measurements

| Backend | Probes | Ping p50 | Ping p95 | Ping p99 | JSON records |
| --- | ---: | ---: | ---: | ---: | ---: |
| iroh | 100 | 4.402 ms | 16.998 ms | 21.399 ms | 117 |
| noq | 100 | 4.054 ms | 7.646 ms | 9.580 ms | 112 |

All observed local dropped/oversize/write-error counters were zero.

Both 100-probe runs use JSON telemetry and the existing rds-bench world. They
measure one established same-host direct connection, excluding five warmups.
The admin listener is not active in these worlds: these are transport smoke
measurements, not an admin-overhead A/B study, connection setup result or WAN/NAT
qualification. The harness prefers loopback while retaining endpoint discovery
defaults. Raw reports are embedded unchanged in the receipt; their `git=8aefccd`
identifies the base, while source hashes identify the tested implementation.

## Remaining work

O2 remains partial: durable policy/lease details and catalog inventory must be
published by transaction owners without scrape-time disk I/O; finer task/queue
coverage and a truthful upstream relay adapter remain. O3 adds phase/reason
correlation and negotiated operation IDs. O4 adds exact-build diagnostics,
redacted bounded support bundles and dashboards. O5 qualifies private rollout,
TLS/roles/rotation/retention, expected instances and independent alert/liveness
paths. O6 covers load/latency/resource campaigns and native macOS.

The [execution sequence](../observability.md#next-reviewable-increments) gives
acceptance criteria for these increments. Native SSH/PTY, desktop capture and
presentation, sync, transport recovery and release gates remain open. This
change neither activates production services nor establishes complete RDS
readiness. No remote CI, native macOS or registered wave-close checkpoint was
run; cargo-deny is not installed locally.
