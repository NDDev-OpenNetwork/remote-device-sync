# Explicit latency path preference — 2026-10-06

W1/W10 path-policy increment. Iroh1.3's default selector always favors a usable
direct path over a relay regardless of relative RTT, and selection callbacks
run after topology events rather than continuous RTT changes. This can prevent
a lower-delay permitted path from serving interactive work.

Endpoint latency preference adds kind-neutral minimum RTT and5ms stickiness,
with opt-in bounded actor refresh. Backend defaults remain; single-path pins
and transport eligibility dominate preference. Noq already periodically ranks
validated paths by RTT. No identity/authentication/wire/OS-route change.

Exact published Iroh1.3 source, MIT/Apache-2.0 licenses and the additional
BSD-3-Clause Tailscale-derived notice are retained with the
small default-None refresh hook; intervals are clamped250ms..60s and unchanged
selection is not reapplied. The patch is a temporary substrate repair, not a
QUIC/TLS fork or completion of owned-Noq migration. No new third-party package.

Tests specify faster relay choice while direct remains usable, stickiness/ties/
unknown current/extreme durations, strict configuration lowering, real actor
refresh without topology changes and termination on endpoint close. Existing
real UDP-proxy packetization tests now select latency with a strict single-path
pin, ensuring that preference cannot escape an impairment route. Qualification
is pending; no deployed device configuration changed from this source yet.

The existing bounded desktop observer records initial selected transmit paths
and actual selection changes at info level, with numeric connection-local path
IDs, kind and sampled RTT. Routine RTT/counter changes do not emit these records.
No address, ticket, input text or credential is logged. The observer retains its
weak transport reference, one worker and newest-only queued sample; no extra
metadata query is added to input/control readers. Tests distinguish initial,
changed, disappeared and reordered selections from ordinary RTT updates.

Refresh skips missed ticks after suspension/stall; obsolete reselection polls
must not burst and compete with current input/media work. An isolated real actor
test stalls one callback long enough to miss multiple periods, checks that they
are not replayed in a burst, then proves normal traffic and endpoint cleanup.

Before that observation follow-up, strict desktop/owned-backend Clippy and all
170 network tests passed locally on both supported platforms. Optimized agent
builds also passed. Supply-chain and native packaging CI passed; the full CI
matrix and observation follow-up are still required at the final source head.
