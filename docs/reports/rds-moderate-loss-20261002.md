# Moderate-loss media starvation — 2026-10-02

An installed native viewer controlled an isolated remote color target with two
actual UI clicks. Both changes appeared in a read-only window capture. Apparent
automation-dispatch-to-capture times were about 1.75 and 4.78 seconds; the two
automation/capture clocks were not independently aligned, and capture itself
adds overhead. These are diagnostic observations, not physical display latency
or a statistically qualified performance result. Cross-host wall clocks are
not subtracted.

Independent records showed a decoded-image age near 4.7 seconds, continuing
packet-loss reductions to a 100 kbps encoding target, and a 63,560-byte recovery
picture whose media acknowledgement took 3,788 ms. Native decode scheduling
reported no slow call in this episode. Timely input injection and ordinary RTT
could coexist with a long picture pause. This identifies a media-rate/recovery
problem in this episode; it does not explain every earlier freeze.

The application bitrate controller now treats sampled loss above 2% through
10% as a path-estimate hold. It retains the minimum packet sample and severe
burst shortcut. Fresh successful media receipts allow at most 1% recovery per
pacing step after the existing hold; absent receipts permit no increase. Severe
loss, sustained RTT growth, producer deadline failures and actual blocked media
retain their reductions, cooldowns, floor and negotiated ceiling. QUIC continues
to control packet congestion and retransmission.

A deterministic replay of successful delivery with continuing 3% or 8% loss
collapsed the old controller to its floor. The regression rejects that outcome.
Additional coverage checks partial-window holds, clean/path-change resets,
receipt-required floor recovery, the 1% probe bound and the negotiated ceiling.
Existing severe-loss and blocked-delivery tests remain required.

This advances W6 media adaptation and diagnostics. Software checks and installed
follow-up results are recorded separately; native visible latency, Full HD
quality and sustained mixed-load stability remain open. No checkpoint closes.
