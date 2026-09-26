# Alert query qualification follow-up — 2026-09-26

Scope: W10.2 test reliability and evidence. No wave or deployment is closed.

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

The test still checks the real pinned Vector/OpenObserve pipeline, remote-write
metrics, scheduled local webhook, and collector/backend restart recovery. It
uses only disposable synthetic peers, credentials and destinations. This is
test-infrastructure hardening, not evidence for production alert delivery,
desktop readiness, a new release or completed W10 acceptance.
