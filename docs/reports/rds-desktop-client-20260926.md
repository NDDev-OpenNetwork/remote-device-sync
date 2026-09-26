# Desktop client task and receive budgets — 2026-09-26

Scope: W6.2 and the client portion of W6.1, plus W2.5/W2.6 lifecycle/deadlines.
No remediation wave is closed. Code source: `ca475e916c23e74f5c4843fceba6b0fcfd694285` (signed task commit).
The [contract](../desktop-client-lifecycle.md) records limits and remaining work.

## Changes

The previous client detached its control writer and one task per incoming
frame. Dropping a session did not own those workers, and a failed duplicate
uni claim happened after tasks had already started. The client now claims
first, bounds the complete handshake, and owns all asynchronous work through
task groups. Control EOF/error also ends sibling work while the public handle
and transport may remain alive.

Four receive/decode inputs per session and eight per process bound encoded
work; excess streams stop before body allocation. Native decoding runs on at
most two blocking workers and keeps its permits through cancellation. Its
result cannot publish to a canceled async receiver. A frame's payload is moved,
not copied into another full buffer.

Unrepresentable next sequence numbers and zero/oversized dimensions are refused
before body reads. Decoded dimensions must match the header; the Rust BGRA
allocation is bounded. Gaps and decode failures require a new keyframe, with a
shared automatic IDR rate limit that permits the first request at time zero.
No wire tag, external helper or third-party dependency changed.

## Local evidence

Linux x86_64, Rust 1.98.1, two build jobs and one Cargo operation at a time.
Only disposable loopback peers were used; installed agents were not replaced.

- Formatting and workspace clippy (default and `rds-desktop/x11`, warnings
  denied) passed.
- Default desktop tests: 14 unit tests, one lifecycle scenario exercising both
  iroh/noq, and seven session/media scenarios passed.
- X11-feature desktop tests: 20 unit tests, the same lifecycle scenario and
  seven session/media scenarios passed. Native OpenH264 encoding/decoding is
  exercised without relying on a display. The capture test can skip when no
  X11 server exists; this is not native capture/presentation qualification.
- Lifecycle coverage: duplicate claim produces no second greeting; incomplete
  streams saturate the limit and excess streams are stopped; control heartbeat
  survives saturation; drop/remote EOF/handshake cancellation release readers,
  control streams and inbox; valid frames still arrive after malformed headers;
  the connection remains usable by unrelated streams.
- Codec coverage checks real keyframe recovery and header/output dimension
  disagreement. Existing seeded loss/jitter, concurrent desktop/sync, queue and
  20-second 60-fps header-soak gates passed in both feature sets. These measure
  synthetic header arrival, not displayed pixels or user-visible latency.

`cargo test --locked --workspace` passed: 546 tests, zero failures, three
opt-in tests ignored, across 94 test/doc-test targets. `cargo-deny` was unavailable
locally; GitHub supply-chain checks remain independent. Exact-head GitHub checks
are required before integration.

A separate serial measurement run on clean source
`ff632db75e128fc10228c71d22ff2729c1d6adbb` passed all seven session scenarios.
The 60-second synthetic iroh loopback soak delivered 3591 headers: age p95 11 ms,
p99 22 ms, test-process peak RSS 49408 KiB. The seeded impaired noq lane delivered
429 headers: age p95 98 ms, p99 107 ms, queue p95 17 ms and heartbeat RTT p95
232 ms. Both directions recorded actual dropped datagrams. These are one-run
supporting measurements, not an improvement claim, process-memory ceiling,
physical-network SLO or displayed-pixel measurement. Reproduction metadata and
binary/lockfile hashes are in the [JSON receipt](rds-desktop-client-20260926-data.json). Native macOS execution and deployment acceptance remain independent.

## Remaining scope

Legacy desktop routing has no per-session ID: a late old tag can collide with a
later inbox claim. Sender `send_frame` cancellation/reference-dependent drops,
global decoded/native codec memory accounting, required native capture/render
tests, graphical window, managed viewer APIs, focus/input release and real
network/platform acceptance remain open. A native decode already running may
finish after cancellation. The receive counters are not process RSS limits.
The published preview is unchanged and no checkpoint command is invoked.
