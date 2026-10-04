# Controlled visual correlation failure — 2026-10-04

W6.8 measurement follow-up. The counter probe previously retained an unanswered
press indefinitely. If a target ignored it, a different much later press could
advance the same counter and produce a false latency sample for the old input.
The negative regression reproduces one false sample after a 188-second gap.

An unanswered press now ends the probe attempt's correlation at five seconds.
Pending samples are canceled and subsequent markers/clicks cannot rearm the
attempt, including across a surface reset. Modified target presses also end
correlation because the target may interpret them differently. Normal desktop
input continues unchanged. The report distinguishes timeouts, modified presses
and lost correlation; these failures must remain visible alongside percentiles.

Regressions exercise the negative scenario, the exact deadline boundary,
permanent failure until a fresh probe instance and modifier invalidation. The
expanded desktop unit suite passed 101 tests. Native installed measurements
still require a verified target, unmodified input and visible presentation;
these diagnostic fixes do not establish low latency or session stability.
