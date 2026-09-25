# Weak metric sampler lifecycle — 2026-09-25

Scope: W2.5 ownership, W0 measurement validity, W10.1 observability.
Source base: `fca82e34fa27fbdfff7fa7262ef165079ee92a6a`. Exact source hashes, toolchain and check commands
are in the [machine receipt](rds-sampler-lifecycle-20260925-data.json).
Linux x86_64 only.

## Reproduced failure and change

Both iroh and Noq regressions failed on the prior code: retaining an idle
sampler prevented peer closure after the application's last Connection was
dropped. A sampler owned a full facade, including its transport and uni router.

Samplers now use weak backend observation handles and metadata. Reads upgrade
only synchronously; no strong I/O handle spans a wait. Weak closure events stop
sampling independently of the interval. Streams remain real I/O owners after
facade drop. Closed iroh handles return an empty live-path snapshot.

Selected RTT/cwnd samples carry a metadata owner token. Drop clears its own
last sample without erasing a newer sampler's observation. The benchmark keeps
its one-shot sampler alive through the scrape. The existing by-value sampler
constructor remains, but transferring the last Connection into it no longer
keeps the connection alive. See the [contract](../path-telemetry.md).

## Validation

All eight final checks passed: workspace/shared formatting, default/X11/all-
feature workspace Clippy with `-D warnings`, workspace tests, expanded
all-feature net/relay/agent/CLI/server tests, and an isolated iroh-only lifecycle
run with Noq disabled.

- Workspace: **431 passed**, 1 ignored, 74 targets.
- Expanded: **238 passed**, 0 ignored, 45 targets.
- Isolated iroh: **2 passed**, 1 target.
- Focused lifecycle, transport metrics and uni-router suite: **15 passed**.

The new fixtures test both backends, peer closure with an idle sampler retained,
closure while a running sampler waits for a one-hour interval, continued traffic
on streams after facade drop, last-stream release, task cancellation, continued
application datagrams, late start after closure, and two live connections
sharing last-selected gauges. Earlier failing tests are not counted as passes.

## Limits

Manual callers still own sampling and drop; the registry does not independently
watch every connection. Active-connection gauges count live samplers. Path
observations and cumulative deltas retain their documented coverage limits;
final sampling cannot recover retired path counters. Full Noq inventory,
lossless accounting and per-session series remain open. Native macOS and real
network qualification were not run; cargo-deny is unavailable. No dependency,
runtime helper, wire change, measured speedup or wave closure is claimed.
