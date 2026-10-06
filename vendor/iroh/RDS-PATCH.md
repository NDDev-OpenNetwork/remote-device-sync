# Iroh1.3 periodic custom selection patch

Upstream: published crates.io `iroh`1.3.0, checksum
885787b892b5e2507c701f132ecbd45d2bad4bbb75157a19427087c16dadb833.
Original SPDX MIT OR Apache-2.0 license files from exact upstream tag v1.3.0
and the additional BSD-3-Clause Tailscale-derived source notice are retained. No QUIC/TLS/relay wire change.

The published selector only runs after connection/path topology events. Add one
optional default-None selector refresh interval, bounded250ms..60s. The RDS
latency selector requests1s; ordinary/pinned selectors preserve original events.
Unchanged selections are not reapplied during refresh, and missed refresh ticks
are skipped rather than replayed in a burst after suspension/stall. The actor already owns
only weak connection references; refresh does not add connection ownership.
No default behavior change for other selectors. This temporary source patch
keeps the working substrate while the owned Noq backend retains its existing
periodic lowest-RTT policy. Upstream qualification and convergence remain work.

The RDS opt-in latency policy additionally reads the already-public Noq
congestion-controller snapshot, probes an established path, and can retire a
failed path after activating an established sibling in the same connection.
These small `PathSelectionData` methods remain in the same patched source file.
Noq's last-open-path guard and reliable retransmission remain unchanged. No
default selector, controller, handshake, TLS, transport eligibility or wire format
is replaced; the RDS controller adapter delegates the actual congestion algorithm.


The opt-in selector can also restore known relay/custom standbys after
abandonment beside an existing IP path. This uses the same client-only
open/validation machinery, deduplicates pending four-tuples and ignores stale
closed handles. Defaults/pinned opening behavior is unchanged. The selection
loop's intended one-IP invariant is repaired by retaining a live IP explicitly
instead of testing an unchanged event-updated map count for every close.
No connection owner is retained across an await and no new addresses are made.

QuicTransportConfigBuilder additionally forwards the default-false
prefer_same_path_acks option from the vendored Noq-proto patch. RDS enables
it only for latency preference, so standby proof can return on its own path.
See ../noq-proto/RDS-PATCH.md for scheduler scope and provenance.
