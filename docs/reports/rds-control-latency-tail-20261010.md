# Retained control-latency failure — 2026-10-10

Status: investigation open. The [October11 follow-up](rds-control-startup-pacing-20261011.md)
fixes an independently reproduced BBR startup-pacing defect and adds passive
scheduler evidence; it does not establish closure of this retained latency tail.
The latency budget remains unchanged.
Scope: W0.1/W6 control measurement and the stability plan's native rollout gate.

## October 10 macOS recurrence and next experiment

[PR150 CI run 38015685163](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38015685163)
at `89db733` passed the default macOS workspace suite and later failed in the
desktop-feature lane. The unchanged 601-probe cohort reported RTT p50 164 ms,
p95 411 ms and p99 584 ms; 624 frame arrivals had p95 190 ms, p99 321 ms and
queue p95 1 ms. The sync regressions, including the original failing scoped
offer, passed. Source/native packages and supply-chain jobs also passed; this
does not make the whole PR qualification green.

Source review found a profile difference requiring verification before tuning
transport: `SyntheticProducer` generates a sequence prefix plus zero bytes and
labels them H.264. Without `x11`, nonempty bodies are consumed as buffered; with
the feature, `Delivery::decode` invokes OpenH264 and may request repair on failure.
This is a hypothesis about workload differences, not an established cause of
the RTT failure. The investigation branch integrates the exact sync code head
without changing production timing or the 400 ms limit.

Next: probe those exact synthetic bytes against the compiled decoder, capture
the failing feature profile's existing application/decode/transport timing, and
separate decoder behavior, observed socket scheduling and ordered loss recovery.
Keep normal and instrumented cohorts distinct. If the fixture differs by feature,
document and test its intended contract before changing its workload; if no such
difference is observed, retain that negative finding and continue delivery analysis.
Neither an increased deadline nor a passing retry closes the original failure.

## Reproduced feature-dependent fixture error

At clean `664b2a6`, a codec-only probe used the actual `SyntheticProducer` for
1,024 sequence prefixes at each of 256 and 1,024 bytes. OpenH264 rejected all
2,048 payloads: zero decoded or buffered results. The probe accelerates cadence
only; it does not change generated payload bytes or claim network timing.
Two planned instrumented feature cohorts then completed: the first failed at
599 probes with p50/p95/p99 152/445/706 ms; the second passed at 601 probes with
139/346/450 ms. Both results are retained, with no selection by outcome.

A new clean-link regression makes the unintended traffic observable: before the
fix, the first 20 headers included two keyframes although only the initial key
was due. The fake H.264 bytes had provoked an extra codec repair. After the fix,
exactly the one intended keyframe arrives. This is independent of the aggregate
RTT's stochastic result.

The shared protocol/header-arrival harness now takes the existing encoded relay
tap and drains its bounded queue with an owned task. It never sends synthetic
pattern bytes through an optional codec. Input/control, stream ordering, loss,
jitter, seed, frame payload/cadence, probe cohort, percentiles and the 400 ms gate
remain unchanged. Real codec tests and the managed H.264 native fixture retain
the decoder boundary separately. The older relay test is renamed to describe
payload preservation and deferred decode outcomes; accepting `NeedIdr` was never
proof of successful decoding.

This repairs an inconsistent test workload; it does not yet establish the cause
of every retained RTT outlier. Default and feature-profile validation follow.

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

1. Reproduce the original aggregate failure while retaining application and
   transport events. The observed passing cohorts below are not that failure.
2. Distinguish expected ordered-stream recovery, driver scheduling and an actual
   implementation defect before changing configuration or measurement design.
3. If a defect is identified, add a deterministic regression where possible,
   then compare unchanged workloads on both supported platforms. Keep the
   original failure and separate synthetic impairment from physical topology.

[RFC 9002 §6.2.1](https://www.rfc-editor.org/rfc/rfc9002.html#section-6.2.1)
defines PTO from smoothed RTT, RTT variation and maximum ACK delay. That primary
reference predates the requested research cutoff; it does not justify lowering
loss thresholds, changing congestion control or weakening the RDS latency gate
without evidence from this implementation.

## Linux observations and packet correlation

At `239a74c`, the same opt-in application trace completed 601 probes on Linux
x86_64 under a one-CPU qualification budget. The gate reported RTT p50 141 ms,
p95 357 ms and p99 419 ms. Client write p95 was 389 µs and server reply write
p95 was 435 µs; the two delivery intervals had independent p95 values of
253.527 ms and 155.052 ms. Seven monotonic probe observations exceeded 400 ms.
The source was clean before and after the run; no transport policy changed.

A separate diagnostic run added `noq_proto::connection=trace` to the filter.
It completed 601 probes with gate RTT p95 352 ms and p99 433 ms. Its longest
monotonic probe RTT was 576 ms. The packet trace correlates that probe with
missing bytes in the existing reliable control stream:

| Time relative to first transmission | Observed event |
|---|---|
| 0 ms | Packet 1054 carries stream range `[2876,2886)` for heartbeat 314. |
| 175 ms | The following range `[2886,2896)` arrives, but heartbeat 315 is not yet readable. |
| 251 ms | ACK processing reports loss; packet 1060 retransmits `[2876,2886)`. |
| 438 ms | Further ACK/loss processing leads to packet 1067 carrying that same range. |
| 520 ms | The missing range is observed at the receiver; heartbeats 314–317 become readable together. |
| 576 ms | Heartbeat 314's reply reaches the client. |

This establishes ordered-stream blocking behind a missing range in that sample.
It does not establish that PTO caused the original CI failure, or that stream
ordering is defective. [RFC 9000 §2.2](https://www.rfc-editor.org/rfc/rfc9000.html#section-2.2)
requires an ordered-stream delivery capability and permits repeated transmission
of the same stream bytes. The priority of a stream cannot make a later message
readable before a missing earlier range on that stream.

Source review also confirms that `rds-net::wire::write_frame` already admits the
length prefix and payload in one buffer, with a partial-write regression. That
earlier repair must not be repeated or credited as a new solution to this tail.
Raw diagnostic traces stay outside the public module; these synthetic numeric
observations contain no deployment identities. All three diagnostic cohorts
and the original failed CI result remain distinct evidence.
