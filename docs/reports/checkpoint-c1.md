# Checkpoint C1 — 2026-09-21

Verdict: **PASS with deferrals** — owned transport is at parity-or-better
with iroh on every scenario the harness can express today; relay and the
long-soak case are explicitly deferred below.

## Automated checks

- `cargo fmt --check`: PASS
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `cargo clippy -p rds-net --all-targets --features transport-noq -- -D warnings`: PASS
- `cargo test --workspace`: PASS
- `cargo test -p rds-net --features transport-noq`: PASS (5 backend tests,
  incl. socket-mux non-primary accept + datagrams)
- `cargo test -p rds-agent --features transport-noq`: PASS (4 e2e incl.
  agent+CLI on `Backend::Noq`)
- CI (PR history): Ubuntu + macOS green incl. `transport-noq` lanes

## Simulation (turmoil, deterministic)

`noq_partition_then_repair_reconnects`: PASS in ~0.7s — handshake fails
honestly during partition, fresh connect + ping succeed after repair.
Two noq endpoints on simulated hosts via `bind_with_socket` injection.

## Impairment / path migration (the wave's reason)

Scenario `impaired` (client→server through UDP proxy, 5% loss, 50ms
delay, 30ms jitter):

| run | p50 | note |
| --- | --- | --- |
| noq before driver | 136.5 ms | pinned to impaired path |
| iroh | 1.68 ms | migrates to direct path |
| **noq after driver** | **1.80 ms** | migrates — proxy carried only the handshake |

Mechanism: per-connection `connection_driver` (QNT `ADD_ADDRESS` →
`open_path_ensure`; `path_events` → biased-RTT selection, `Available` on
best path, `Backup` on rest, 5ms stickiness). Weak handles only, so
last-handle-drop still closes the connection — verified by the existing
close-path tests staying green.

## Parity suite (same harness, both backends)

noq: `docs/reports/bench-20260921-c1-noq.{json,md}`.
iroh numbers from the same-day suite run (earlier this session):

| scenario | iroh p50 | noq p50 | verdict |
| --- | --- | --- | --- |
| handshake | 25.2 ms | 22.4 ms | parity |
| ping | 2.8 ms | 1.65 ms | parity |
| transfer | 24.8 MiB/s | 25.3 MiB/s | parity |
| multiconnect @ 5%/50/30 | 159.4 ms | 155.6 ms | parity (fresh conns start on advertised path) |
| impaired ping | 1.68 ms | 1.80 ms | parity — migration proven |
| relay-fallback | works | explicit WS2 error | deferred — noq has no relay transport yet |

## Interop

`noq_backend.rs`: iroh client ↔ noq server and noq client ↔ iroh server
on `rds/0` — PASS both directions (RFC 7250 raw-public-key TLS
wire-compatible with iroh's).

## Fuzz

Not run — this wave touched no custom wire decoder. Owned wire formats
(`rds-relay` proto, discovery records) land in WS2/WS3; fuzz gates are
attached to those waves' checkpoints.

## Soak

Not run — the 30-minute path-flap soak is specified but the soak runner
is not built. Deferred to a dedicated evidence run before the `noq`
backend is considered for default.

## Security review

- No `unsafe` added this wave.
- QNT advertisements are limited to our bound socket addresses
  (loopback + kernel egress hint for unspecified binds); cap 32 matches
  transport config.
- Driver tasks are bounded per connection and self-terminating.
- `multiconnect` p95 ≈ 1.3s remains handshake retransmission under 5%
  loss — bimodal, matching iroh's distribution.

## Exit state

- `iroh` remains the default backend (gate: soak + relay parity in WS2).
- `Backend::Noq` is fully usable behind `--backend noq` /
  `transport-noq` for agent, CLI and bench.
