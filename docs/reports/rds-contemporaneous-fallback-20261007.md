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

Source `5169f0f` passed all 18 CI checks (16 successes and two ordinary skips),
including both platform test jobs, native builds, supply-chain and Rust/Actions
CodeQL. Merge `5f2445d` has the identical tree
`46cd77bfaf8c3ec99e8b607566b6ea3adda67a32`. Mac strict desktop/Noq Clippy,
workspace tests, net/Noq integrations and cargo-deny passed with FD4096. Linux
net/Noq, strict workspace desktop/Noq and X11 Clippy, and the desktop-agent
release passed. An initial command used an invalid feature name; the next
workspace run exhausted its inherited FD256 limit. Both environment failures
were retained. The full suite passed with the declared FD4096 profile; no test
was removed or ignored to manufacture success.

The [owned-relay migration harness](bench-standby-20261007.md) measured
19.500375 ms drain recovery and 113.465041 ms kill recovery on isolated loopback
relays using the original connection. These are two recovery samples, not
recovery percentiles, Iroh three-origin qualification or native input/pixels.
Installed correction receipts belong to the private estate; code/loopback
checks do not establish causal native improvement.

Private incident evidence retains source intervals and logging limits;
correlation of route changes and slow input cannot identify every pause's cause.

## Remaining qualification

Observe exact-build real typing/deletion and counts, control tails, foreground
presentation, idle and mixed-load recovery after scoped deployment. Include
unanswered probes and retained failures. A stricter proof cannot create
connectivity when every route is unavailable.
