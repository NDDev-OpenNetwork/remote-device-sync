# Desktop reference-chain qualification — 2026-09-26

Scope: W6.1 increment; no wave, input-to-visible or release gate is closed.
Qualified source: `4221c59a53713de716f4e781a858a3587f991621` (clean tree).
See the [sender contract](../desktop-frame-delivery.md).

A blocked delta payload was abandoned when its dependent successor arrived.
The new 64-byte-backpressure regression failed before the change and now checks
that the full 4096-byte reference reaches the reader exactly once. A successor
delta waits for that reference; an independent keyframe can replace it with
RESET. Final frames still finish when the producer closes.

Queue admission now requests IDR after losing any encoded frame. Queue collapse
is bounded and requests recovery for discarded references, including deltas
after a retained keyframe. A sender sequence guard rejects deltas after a gap,
empty encode result or sequence exhaustion until an independent frame arrives.
Admission occurs after the final post-pacing selection. Queue capacity remains
two; no wire or dependency change is introduced.

## Validation and measurements

Linux x86_64, Rust 1.98.1, two build jobs, one local Cargo operation at a time:

- Formatting and workspace clippy, default and X11, passed with warnings denied.
- Workspace: 555 passed, zero failed, three ignored across 96 test/doc targets.
- X11 unit suite: 28 passed, zero failed; the native display-repainting fixture
  remains explicitly ignored here and required in the separate Linux Xvfb lane.
- Native H.264 encodes seven images with actual IDR/delta flags. After dropping
  one reference, the guard admits sequences 0, 1, 5 and 6, rejects dependent
  deltas 3/4, and checks a decoded pixel before and after the requested IDR.
- Existing real iroh/noq stream RESET and serving/client cancellation tests pass.
- A clean-source serial run passed all seven session scenarios. A 60-second
  iroh loopback delivered 3601 synthetic headers, age p95 3 ms/p99 5 ms, process
  peak RSS 50060 KiB. The impaired noq lane delivered 420 headers, age p95
  110 ms/p99 116 ms, queue p95 37 ms and control RTT p95 159 ms. Both directions
  recorded actual seeded datagram drops.

[Reproduction data](rds-desktop-references-20260926-data.json) contains exact
source, command, toolchain, binary/lockfile hashes and impairment counters.
These are single-run synthetic header measurements, not decoded/presented
latency, a comparative improvement, a native-memory ceiling or WAN acceptance.
The native codec fixture is a separate local test, not real-codec QUIC impairment.

An intermediate FIFO-only prototype preserved references but failed the existing
queue-age threshold: p95 114 ms against 100 ms. It was replaced by bounded
collapse with explicit recovery/sequence admission. The final policy passed a
focused impairment test and the complete suites above. No threshold was relaxed
and no failed test was retried unchanged to manufacture a pass. X11 clippy also
caught a test-fixture chunk API lint, which was corrected before final checks.

The first GitHub Linux run at `b3d910d79cb93d6cb4954e7a171af2afef5a98d2`
([run 36235900357](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36235900357))
stalled in the pre-existing `relay_forwards_handshake_and_datagrams` fixture.
The live job log showed its five sibling scenarios completed and this scenario
still running after more than 17 minutes of job time. Its unbounded awaits did
not identify the stalled phase; this is not evidence of a specific relay-runtime
root cause. The fixture now reports startup/attach/handshake/datagram/stream/close
phases with five-second deadlines, polls connect/accept together without a
detached task, bounds its ACK read and explicitly closes the directory. All
original payload/identity/forwarding assertions remain. One full local run and
a fixed ten-run batch passed all six scenarios (66 executions). This bounds and
improves diagnosis of the qualification test; it does not prove loss-free
DATAGRAM delivery or repair an unlocalized runtime fault. Final GitHub checks
are required for the revised source, not inferred from earlier green jobs.

Remaining: real H.264 network loss/reordering/RESET continuity, long-duration
overload and IDR-rate behavior, pacing/grant convergence, desktop wire session
IDs, managed viewer/rendering, native memory accounting and physical-platform
input-to-visible acceptance. GitHub integration is independently required before
merge. No installed agent or published preview is changed.
