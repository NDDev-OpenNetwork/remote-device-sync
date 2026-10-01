# Reference recovery follow-up — 2026-10-02

The moderate-loss controller was installed and a ten-minute native observation
received 8964 frames, submitted 8324 images and recorded 343 input acknowledgements
without reconnecting. Full HD remained selected. The decoded-image age was
49 ms at p50 and 338 ms at nearest-rank p95; its 5433 ms maximum failed the
continuity criterion. The window was presented in 523 of 597 samples. These are
native diagnostics, not physical display latency or universal stability proof.

Three isolated native UI clicks changed the captured window to their expected
colors. Apparent automation-dispatch-to-capture times were 1433, 579 and 569 ms;
clock alignment and automation overhead remain unqualified. All three remote
fixture events were confirmed; cross-host wall clocks are not subtracted.

The earlier 5.4-second pause retained stale encoded and decoded images. Local
receiver logs reported two expirations of the same reference gap about one
second apart; serving logs showed two roughly 90 KB recovery pictures and a
4387 ms transport acknowledgement. No slow native decode was reported. This
supports investigating redundant recovery in this episode, without attributing
all prior freezes to it.

A real-stream regression over both supported transport backends reproduces
successors arriving after a requested recovery. Old behavior queues another IDR
for the same episode. New behavior coalesces it, then proves a later missing
reference after a received key still requests recovery. Deterministic cases
also preserve retry at the existing key-reader bound, failure reset and retry
when control admission was full. Native qualification after this change remains
separate and open.

Successful slow-read diagnostics and scoped native-input timing address two
measurement gaps: successful multi-second frame reads used to be quiet, and
an automation dispatch clock was being used as the action origin. Diagnostics
contain no input values, screen contents or peer identities. This advances W6
media recovery and measurement; no checkpoint closes.
