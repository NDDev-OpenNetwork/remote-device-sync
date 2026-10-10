# Control recovery and startup pacing — 2026-10-11

Scope: W0.1/W6, shared transport correction and C5 acceptance.
Status: BBR defect reproduced and fixed; original CI control-tail cause remains
open. [Machine results](rds-control-startup-pacing-20261011.json) retain all six
predeclared cohorts. No deadline, loss threshold, seed or sample was weakened.

## What the high delay measures

[Main CI 38077078849](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38077078849)
failed at 601 completed probes: control RTT p50/p95/p99 213/521/678ms versus the
400ms p95 budget. Queue p95 was 1ms; that result is retained in the
[earlier investigation](rds-control-latency-tail-20261010.md).
The fixture deliberately adds 50ms plus 0–29ms sampled jitter **per direction**
and drops 5% of datagrams per direction. These are header/control measurements,
not physical input-to-visible pixels or a live WAN performance claim.

One unchanged baseline sample (heartbeat 190) took 854ms. Its request packet
arrived at 76ms, but an earlier missing stream range blocked application read
until 481ms. The server's reply took 351us to write; earlier missing reply bytes
then held it for another 372ms. Later replies became readable together after
retransmission. This establishes ordered recovery in that sample, consistent
with [RFC9000 §2.2](https://www.rfc-editor.org/rfc/rfc9000.html#section-2.2) and
[RFC9002 §6](https://www.rfc-editor.org/rfc/rfc9002.html#section-6). It does not
identify the original CI percentile's cause. Independent leg percentiles or
maxima must not be added.

## Reproduced implementation defect

BBRv3 initialized pacing using a nominal 1ms RTT, then only raised it while
STARTUP had not filled the pipe. An application-limited flow could retain that
nominal rate indefinitely. The deterministic 40ms regression observed
33,276,000B/s instead of 831,900B/s. All three new regressions failed before the
correction and passed after; the complete engine suite passed 433 tests.

The shared engine now calibrates once from a tracked packet's measured RTT,
including the first BBR sample. Noq updates its transport estimator **after**
congestion callbacks, so its initial/validation estimate cannot substitute for
that first measurement. A zero sample is floored at 1us; sub-ms paths, controller
clones, ordinary STARTUP growth and send-quantum updates are covered. Both Iroh
and owned Noq use this engine. No new protocol, estimator API, controller choice
or application retransmission was added.

Primary sources: [BBR draft-06 §5.6.2](https://www.ietf.org/archive/id/draft-ietf-ccwg-bbr-06.html#section-5.6.2),
[Google BBRv3](https://github.com/google/bbr/blob/v3/net/ipv4/tcp_bbr.c),
[Noq PR802](https://github.com/n0-computer/noq/pull/802) and reviewed
[moq-dev/noq PR7](https://github.com/moq-dev/noq/pull/7), merged September 25.
The narrow RDS adaptation and published-engine provenance are recorded in
[the vendor note](../../vendor/noq-proto/RDS-PATCH.md).

## Predeclared comparison

Three consecutive baseline runs at `0e17aab`, followed by three consecutive
runs of the recorded pacing patch. Each retained the same 60sec workload,
100ms probes, 60fps/1024-byte opaque frames, both seeds and impairment. Both
series used the same opt-in application/packet trace, ran without concurrent
Cargo/timing work and retained every outcome. The fixed patch digest in JSON
binds the tested source; the signed correction is `446629d` (subsequent comment
and documentation changes do not change its production behavior).

| Cohort | Probes | Gate p50 ms | Gate p95 ms | Gate p99 ms | Max reader RTT ms | Reader RTT >400ms |
|---|---:|---:|---:|---:|---:|---:|
| baseline 1 | 601 | 141 | 329 | 405 | 555 | 7 |
| baseline 2 | 601 | 140 | 346 | 402 | 610 | 7 |
| baseline 3 | 601 | 144 | 391 | 609 | 854 | 27 |
| pacing-fix 1 | 601 | 141 | 349 | 441 | 544 | 9 |
| pacing-fix 2 | 601 | 141 | 371 | 522 | 595 | 18 |
| pacing-fix 3 | 601 | 141 | 363 | 468 | 681 | 11 |

All six aggregate gates passed, but their distributions overlap. This does
**not** demonstrate aggregate control-tail improvement or reproduce/close the
521ms CI failure. Trace can perturb scheduling; a fixed RNG seed does not fix
the runtime/QUIC packet schedule. Reader RTT is recorded before test event
observation and may differ slightly from gate RTT. Raw logs and failed
regressions remain outside the public module; JSON includes content digests.

## Better failure evidence

The socket decorator now records three bounded passive maxima: delay-heap depth,
release lateness relative to the packet's sampled release time, and inner-socket
send wait. No new queue, timer, task or payload history is retained. Rate pacing
contributes to release lateness; it is distinct from configured propagation and
from peer reader scheduling. A real UDP regression covers complete loss followed
by delayed, byte-exact delivery with unchanged counters.

The acceptance test prints those observations and p95/max probe-tick lateness
before asserting the original gate. Tracing initialization errors are explicit.
The complete summary is formatted before printing so packet trace writes cannot
split its format arguments. These observations will distinguish a delayed local
pump from ordered recovery on a failing runner; they are not proof about the
old uninstrumented failure.

## Remaining acceptance

Run the registered transport checkpoint and both supported CI platforms at the
final source. Preserve any new failed cohort with its scheduling observations.
If the CI tail recurs, correlate that failed runner's application/packet timing
before making another transport change. Keep Noq experimental and physical
mixed-load/native topology qualification separate. Installed runtime must keep
its original build provenance until a separately qualified update.

## Checkpoint calibration defect retained

The first registered C1 run passed workspace formatting/strict Clippy/tests,
owned-transport checks and deterministic simulation, then failed its calibration
transfer. [The original failed suite](bench-20261010-203148-noq.json) is retained.
The preexisting scenario reused the 32 MiB bulk payload with a 10 Mbps cap and
15-second deadline: payload serialization alone requires 26.8435456 seconds.
That configuration cannot complete even on an ideal transport; it is separate
from the BBR change and the 400ms control gate.

Calibration now owns `--calibration-mib` (default 4 MiB); the ordinary transfer
retains 32 MiB. Its 15-second deadline and 0.4–1.2 goodput ratio remain. Invalid
rate/size/deadline combinations are refused before backend selection or endpoint
creation. Regressions preserve bulk size, reject the original impossible budget
before an invalid backend is reached, and cover invalid numeric inputs. The
changed registered checkpoint must complete before a passing receipt is emitted.
