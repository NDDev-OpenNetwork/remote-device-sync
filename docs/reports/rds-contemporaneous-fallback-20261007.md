# Contemporaneous sibling proof before retirement — 2026-10-07

Scope: W3.6/W6.8, opt-in latency policy. This correction does not close a
network/native stability gate.

An idle sibling's positive ACK could remain generally fresh while predating the
failed path's outstanding-work interval. The previous retirement condition
accepted it as a replacement. A common outage could therefore authorize
abandoning reliable STREAM work without proof that the sibling still progressed
during that outage. A larger RTT enlarged the freshness window without making
that earlier ACK contemporaneous.

Shared `Observation::can_replace` requires a sibling ACK strictly after the
failed pending interval began, plus existing freshness and nonstalled conditions.
Both Iroh and owned Noq retirement use it. Ordinary RTT ranking, probing, engine
loss detection, congestion algorithms, priorities, identity, wire formats and
last-path guards retain their behavior. Missing proof cannot authorize retirement.

## Reproduction and evidence

`replacement_requires_progress_after_the_outstanding_work_interval_began`
failed with the former predicate. It now rejects earlier/equal confirmation,
accepts later proof, and rejects a stalled/expired sibling or absent failed
interval. The high-RTT case rejects old replacement proof while the general
freshness predicate still considers it recent.

macOS arm64 `cargo test --locked -p rds-net --features transport-noq --lib`
passed all 110 tests. Existing real-engine tests retain Iroh actor blackhole
recovery, standby restoration before a later failure, Noq original-stream
pending-byte recovery, backup ACK routing, probe-only debt and runtime clocks.
The new tests supply the negative proof boundary.

Broader workspace/features, Linux/X11, supply-chain and CI checks are being
collected. No installed correction or causal improvement is asserted from this
unit run. Private incident evidence retains source intervals and logging limits;
correlation of route changes and slow input cannot identify every pause's cause.

## Remaining qualification

Observe exact-build real typing/deletion and counts, control tails, foreground
presentation, idle and mixed-load recovery after scoped deployment. Include
unanswered probes and retained failures. A stricter proof cannot create
connectivity when every route is unavailable.
