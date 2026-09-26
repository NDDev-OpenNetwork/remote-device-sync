# Collector delivery and alert qualification — 2026-09-26

Scope: W10.2 collector delivery, recovery and qualification. No wave or
deployment is closed.

The main build at `b1e772ba292773cf00afb45d0f1994ab06725d23` passed
[Linux/macOS Rust CI](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36228939113),
[supply-chain checks](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36228939188),
[CodeQL](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36228939120)
and [artifact packaging](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36228938739).
Packaging on main does not publish a release.

Its separate [observability qualification](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36228939141)
failed while waiting for the exact 20% connection-error boundary. Earlier log,
metric and authenticated admin-scrape assertions passed. The same baseline
passed locally, and a diagnostics-only change passed both locally and in a
[GitHub diagnostic run](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36229327595).
The original failure's cause is **not proven**; the passing rerun does not
establish that it has been fixed.

The qualification now requests uncached searches and refuses partial responses
or a nonzero reported cache ratio. OpenObserve documents the cache switch as
the [`use_cache=false` URL parameter](https://github.com/openobserve/openobserve-clickhouse-benchmark/blob/main/scripts/run-benchmark.py).
This removes cache reuse as an acceptable source of qualification evidence; it
does not assert that caching caused the original failure.

Each synthetic fixture phase waits for its exact ingested record count before
checking the production SQL. The test checks fewer than ten attempts, exactly
two failures out of ten, duplicate delivery and cancellation at that boundary,
and recovery to two failures out of eleven. Duplicate records must actually be
visible before deduplication is tested. The production threshold, alert SQL,
polling deadline and scheduled webhook contract are unchanged. On another
boundary failure, diagnostics include synthetic raw rows and aggregate counts.
An initial uncached local run of the expanded test also stalled at ten records
while waiting for twelve; a subsequent diagnostics-enabled run passed. Collector
logs and buffer/error metrics are now included on ingestion timeout. This
delivery uncertainty remains under investigation rather than being attributed
to query caching or hidden by a longer timeout.

Focused formatting, clippy and all 25 `rds-observe` unit tests passed locally.
A fixed three-run diagnostic batch of the expanded pipeline then passed all
three times. These results strengthen regression coverage but do not disprove
the recorded delivery stall.

The [first PR pipeline run](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36230012556)
stopped before Rust tests because the public image registry returned
`toomanyrequests: Rate exceeded`. CI now makes at most three pulls of each
unchanged digest, with 0/10/30-second backoff and a 120-second cap per pull.
Failure still fails the job; there is no mutable-tag fallback or test retry.

## Delivery boundary and replacement

Further GitHub runs reproduced both missing initial ingestion and a stopped
single-record tail. The expanded local eight-tail regression reproduced the
latter with independent Vector metrics: the log buffer had received 30 events,
sent 29 and retained one event (576 bytes) beyond the unchanged 30-second
deadline. Backend searches were healthy and complete. This localizes the
observed stall to the collector delivery path; it does not identify a specific
upstream internal race or prove that every older failure had this cause.

The shipped file-source configuration now uses a bounded 4096-event memory
queue with blocking backpressure and sink-level acknowledgements. Retained
source files and persisted acknowledged checkpoints provide replay. No image
version, alert threshold, polling deadline or Rust product dependency changed.
There is no periodic dummy event or collector restart used to unstick delivery.

Recovery qualification kills Vector with SIGKILL while OpenObserve is stopped,
then checks all seven unique new records after restart. Before killing it, an
independent loopback scrape must prove the records reached the collector.
Eight successive single-record appends must each become searchable without a
later append rescuing them. This tests idle delivery and actual crash replay,
not merely eventual batch ingestion after graceful shutdown.

The new durability boundary requires retaining unacknowledged files, including
rotations. Existing disk-buffer installations must drain and verify pending
records before switching types; no existing deployment is changed here. Global
retention/quota operations, physical power-loss and production alert routing
remain open. See the [current contract](../observability.md).

The test still checks the real pinned Vector/OpenObserve pipeline, remote-write
metrics, scheduled local webhook, and collector/backend restart recovery. It
uses only disposable synthetic peers, credentials and destinations. This is
collector configuration and test hardening, not evidence for production alert delivery,
desktop readiness, a new release or completed W10 acceptance.
