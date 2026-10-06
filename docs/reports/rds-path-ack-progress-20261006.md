# Acknowledgement progress during path selection — 2026-10-06

This W6.8/W10 increment handles a validated path whose old RTT remains attractive
while new reliable data no longer receives acknowledgements. RTT alone is a
measurement of prior delivery; received bytes are also insufficient because
multipath ACKs may arrive on a different path.

## Policy and ownership

Only explicit `PathPreference::Latency` installs the passive controller adapter
on both backends. Every controller callback, congestion window, pacing metric,
MTU and ACK-frequency update delegates unchanged to the configured BBRv3/Cubic
controller. Outstanding ack-eliciting work begins a proof budget; fresh positive
ACK progress advances it, and an empty flight ends it. Idle time does not consume
a new send's budget. Snapshots preserve evidence, while a live controller clone
starts a new proof domain on its first mutation. Runtime clock injection avoids
comparing callbacks against an unrelated wall/virtual clock.

No ACK progress for `max(4 RTT, 500ms)` makes a path ineligible while a healthy
alternative exists. A sibling's positive ACK must be younger than `max(8 RTT,
2s)`, and it must have no overdue work. Idle candidates receive standard probes
to establish/renew that evidence. The original5ms switching stickiness remains
for healthy paths. A high RTT cannot be truncated into premature failure by a
fixed timeout. Unknown instrumentation retains the prior policy.

Retirement activates the confirmed established sibling in the same connection
before closing the failed path. Noq refuses closing the last open path; the
connection, endpoint, application stream, authorization and input sequence are
retained. This is not input replay or QUIC loss/crypto reimplementation. Iroh's
three small context methods remain in the existing single-file vendor patch.
Ordinary/default and pinned selectors preserve their behavior.

## Native isolated evidence

The Noq fixture uses real IPv4/IPv6 loopback sockets, authenticated connections,
a validated standby and a bounded switch that silently drops outgoing primary
packets after handshake. It proves a pending stream frame was emitted on that
path before switching. Manual retirement delivered the already sent22bytes in
the same stream; the automatic policy also recovered without reconnecting.

The actual Iroh actor fixture starts over a bounded64-packet custom link, learns
its UDP standby, then drops the preferred custom link. Its existing stream
delivered the pending22bytes via the actor/policy in about1.1seconds. The fixture
uses Iroh's declared custom-transport types; `iroh-base`1.3.0 and `n0-watcher`1.0.0
are test-only direct dependencies on already resolved transitive packages.

The first expanded Mac unit run passed100tests, including both native recovery
fixtures and controller-clock/clone/idle boundaries. A high-RTT boundary was
added afterward. Final exact-source fmt/strict Clippy, complete regression,
Linux lane and installed two-device qualification are still pending. An earlier
full-workspace link failed on temporary-cache ENOSPC after Clippy had passed;
only owned inactive generated artifacts were removed, and subsequent checks use
a disk guard. The original failure is retained, not reported as passed.

No deployment or full latency/stability acceptance is claimed by these fixtures.
They establish bounded same-connection recovery for this specific failure mode.
