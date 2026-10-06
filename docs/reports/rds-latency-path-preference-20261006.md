# Explicit latency path preference — 2026-10-06

W1/W10 path-policy increment. Iroh1.3's default selector always favors a usable
direct path over a relay regardless of relative RTT, and selection callbacks
run after topology events rather than continuous RTT changes. This can prevent
a lower-delay permitted path from serving interactive work.

Endpoint latency preference adds kind-neutral minimum RTT and5ms stickiness,
with opt-in bounded actor refresh. Backend defaults remain; single-path pins
and transport eligibility dominate preference. Noq already periodically ranks
validated paths by RTT. No identity/authentication/wire/OS-route change.

Exact published Iroh1.3 source and MIT/Apache-2.0 licenses are retained with the
small default-None refresh hook; intervals are clamped250ms..60s and unchanged
selection is not reapplied. The patch is a temporary substrate repair, not a
QUIC/TLS fork or completion of owned-Noq migration. No new third-party package.

Tests specify faster relay choice while direct remains usable, stickiness/ties/
unknown current/extreme durations, strict configuration lowering, real actor
refresh without topology changes and termination on endpoint close. Existing
real UDP-proxy packetization tests now select latency with a strict single-path
pin, ensuring that preference cannot escape an impairment route. Qualification
is pending; no deployed device configuration changed from this source yet.
