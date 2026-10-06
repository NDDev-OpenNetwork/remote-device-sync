# Controlled typing before focus-click presentation — 2026-10-06

This W6.8/W10 diagnostic correction preserves causal counter matching when a
controlled typing burst begins before the focus click's response is submitted.
It changes only the opt-in visual measurement, not input dispatch, transport,
codec, session lifecycle or the latency of actual input.

## Reproduction and correction

Given presented counter0, a target click expects counter1. Previously, keys
received before the click's counter reached GPU submission were discarded as
unanchored. After counter1 armed measurement, later keys were assigned counter2
and beyond, even though the discarded earlier keys had already advanced the
target. That could report earlier characters as responses to later presses,
producing underestimated latencies before an unrequested-counter failure.
Every affected attempt is invalid; its small partial measurements cannot qualify
latency even when application event counts are correct.

The measurement retains controlled keys provisionally behind an already anchored
pending target click. Counter matching remains one ordered sequence; keyboard
arming still requires the click response in an actually submitted frame.
Focus/surface loss, modifiers, the five-second response timeout and the existing
128-pending limit preserve their boundaries. No input is replayed or delayed.

Deterministic regressions cover a click followed by two keys before presentation:
the counter1 frame confirms only the click; counter3 at180ms reports key delays
of170/160ms. A collapsed counter33 frame confirms all32 fast keys after a click;
focus loss cancels all33 pending measurements. A timed-out focus click cancels
its provisional typing and permanently fails that attempt.

## Validation boundary

The corrected reader of controlled counters measures input-to-GPU-submission,
not physical scanout. [Apple's presentedTime API](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime)
provides a separate display-time boundary, and dropped drawables report zero.
Network ACKs and decoding alone cannot satisfy this probe.

The existing full-owned transport/media qualification remains open. Increasing
bitrate does not follow from delayed control messages: [RFC8290](https://www.rfc-editor.org/rfc/rfc8290.html)
describes queue isolation for interactive traffic; [RFC9002](https://www.rfc-editor.org/rfc/rfc9002.html)
retains loss/PTO recovery independently of media bitrate. This correction supplies
trustworthy evidence for diagnosis and does not claim to fix physical-network
stalls.

Final source checks and installed-device verification are recorded separately;
this report is not a wave-close checkpoint.
