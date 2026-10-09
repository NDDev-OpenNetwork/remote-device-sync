# Retained control-latency failure — 2026-10-10

Status: investigation open; no latency budget or production transport change.
Scope: W0.1/W6 control measurement and the stability plan's native rollout gate.

[Main CI run 37999738254](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/37999738254)
at `6825d5d` failed the Ubuntu impairment case with 601 completed probes:
RTT p50 174 ms, p95 445 ms and p99 683 ms versus the unchanged 400 ms p95 budget.
The workload retained 5% loss, 50 ms one-way delay, 0–30 ms added jitter and the
existing seed. Increasing the cohort in PR145 improved measurement coverage;
it did not repair this runtime tail. Successful later runs cannot erase it.

## Diagnostic observation

Test-only `5940206` enables the existing `rds_desktop::control_timing` probes
when requested through `RUST_LOG`. The workload, seed, cadence, sample handling,
percentile calculation and threshold are unchanged. One macOS arm64 run used:

```sh
RUST_LOG=rds_desktop::control_timing=trace \
  cargo test --locked -p rds-desktop --test session_v2 \
  impaired_link_latency_gate -- --exact --nocapture
```

All 601 probes completed. The test reported p50 140 ms, p95 375 ms and p99
482 ms; 18 per-probe monotonic RTT observations exceeded 400 ms. This run passed
the aggregate gate and is not a reproduction of the failing Ubuntu cohort.

Matching all five application timing events by sequence gave these independent
distributions (their percentiles must not be added):

| Interval | p50 | p95 | p99 | Maximum |
|---|---:|---:|---:|---:|
| Client control write | 244 µs | 360 µs | 486 µs | 806 µs |
| Server reply write | 55 µs | 114 µs | 247 µs | 4,364 µs |
| Client write completion → server read | 69.210 ms | 273.920 ms | 314.402 ms | 420.590 ms |
| Server write completion → client read | 70.502 ms | 217.571 ms | 251.747 ms | 445.234 ms |

The timestamp span and the client's monotonic probe RTT differed by at most
1.003 ms in the positive direction. The long delays in this traced run occurred
between application write completion and peer read, while the writes themselves
were short. That narrows the investigation to delivery/transport and task
scheduling; it does not identify a particular lost packet, PTO, ACK policy or
congestion-controller defect. Trace capture can itself perturb scheduling.

## Next verification

1. Capture the same per-sequence observations on Linux, retaining both passing
   and failing cohorts and the actual packet/loss counters. Do not select only
   runs that meet the budget.
2. Correlate delayed delivery with transport ACK/loss/retransmission and driver
   scheduling before changing configuration. Application timestamps alone do
   not prove network-only delay.
3. If a defect is identified, add a deterministic regression where possible,
   then compare unchanged workloads on both supported platforms. Keep the
   original failure and separate synthetic impairment from physical topology.

[RFC 9002 §6.2.1](https://www.rfc-editor.org/rfc/rfc9002.html#section-6.2.1)
defines PTO from smoothed RTT, RTT variation and maximum ACK delay. That primary
reference predates the requested research cutoff; it does not justify lowering
loss thresholds, changing congestion control or weakening the RDS latency gate
without evidence from this implementation.
