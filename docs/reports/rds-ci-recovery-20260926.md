# CI image and recovery-check qualification — 2026-09-26

Scope: W0 regression accuracy and W10 collector qualification. No runtime,
wire or release change, and no wave acceptance is claimed.

## Registry

The [main collector run](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36231833331)
stopped before tests after three ECR `Data limit exceeded` responses.
OpenObserve's `o2cr.ai` endpoint served the expected manifest locally but
[refused anonymous GitHub pulls](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36232346970).
The upstream [Docker Hub repository](https://hub.docker.com/r/openobserve/openobserve)
serves the exact same pinned v1.0.4 multi-platform digest and amd64/arm64 child
manifests. `images.env` now uses that address; neither image version changed.
The unchanged pull deadline/retry cap still fails closed, without mutable tags
or test retries. Public GitHub-hosted runners remain in use.

The complete disposable Vector/OpenObserve regression passed locally with
these image contents and on [GitHub at `0ee92e1`](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36232540598),
including logs, metrics, alerts, isolated tails and SIGKILL checkpoint replay.
That is collector evidence, not success of the entire CI matrix.

## Relay recovery assertion

The [same head's macOS Rust job](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36232540640/job/108378332493)
failed the sequential DATAGRAM echo after relay loss. Diagnostics recorded
24 requests sent/received but 23 replies; both connections stayed open and their
direct paths were Available. A further synthetic path remained present. The
specific cause and full retirement of engine-created paths are not established
by this test correction and remain part of W3 qualification.

[QUIC DATAGRAM is unreliable](https://www.rfc-editor.org/rfc/rfc9221.html#section-5.2).
Waiting forever for each individual reply conflates a lost datagram with a
broken connection. The recovery gate now opens a bidirectional service stream
before relay shutdown and verifies 25 byte-exact roundtrips on that same stream
afterwards, matching SSH/control/sync delivery semantics. The three-second
traffic budget, relay STREAM evidence, one-second selection check, failed-ticket
refusal, advertisement removal and joined endpoint cleanup remain unchanged.
There is no application retry or relaxed timing threshold. Zero-loss datagram
handover is not asserted or claimed to have been fixed.

Test source: `4286809ca3d6611b6066f3c13d4d52ec05102f99`. Focused owned-relay clippy passed;
the updated two-scenario target passed one initial run and a fixed ten-run batch
(20 scenario executions) on Linux. The old test also passed one local run; that
does not invalidate the observed macOS failure. Exact-head GitHub qualification
remains independent, and this change does not prove physical-network latency
or complete every pending path-retirement case.
