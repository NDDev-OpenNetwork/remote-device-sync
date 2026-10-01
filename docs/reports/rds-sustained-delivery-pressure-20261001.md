# Sustained delivery pressure — 2026-10-01

This follow-up advances W6.6 and W10 diagnostics. It does not close broad native
platform, hardware, physical-pixel latency or complete impairment gates.

## Observed failure

The previous adaptation reacted to each soft delay report, even after that
frame had completed and other receipts continued. Sparse jitter repeatedly
restarted a five-second recovery hold and reduced the encoder toward its floor.
A large recovery keyframe at that low target then caused a codec-skip drought.
Positive native input checks did not establish longer-run stability; the failed
longer observation remains in the private consumer evidence.

## Change

Each late transport receipt owns a small RAII guard. Its current-pending count
is bounded by the existing three ACK tasks and released on completion, error
or task cancellation. Cumulative delayed counts remain diagnostic history.
The pacing task identifies soft pressure only after two consecutive250 ms
samples with a late receipt still outstanding and no new successful receipt.
Hard delivery failures still react immediately; path loss/congestion/RTT and
capture deadline signals retain their existing response. Bitrate floor, grant
ceiling, five-second recovery hold, one-second cut coalescing, slow upward probe,
frame ACK/read deadlines and capture admission are unchanged.

Normal health records add current late receipts and stalled-sample count. These
are bounded metadata, with no payload, peer identity or clipboard content.
No wire, authorization, dependency or platform API changes are introduced.

## Regression evidence

A real UDP fixture pauses server emission for a brief interval, then restores
healthy delivery and observes multiple pacing ticks. The old implementation
unnecessarily reduces bitrate and fails; the new implementation preserves it.
The existing longer blocked-ACK fixture still bounds capture to one held key,
reduces load despite clean transport counters, and resumes on the same connection.
Both targeted cases pass. Additional tests cover two-sample detection, immediate
hard failures, repeated jitter with continuing receipts, and cancellation of an
owned late-receipt task. Current whole-suite and installed-device results will
be recorded after qualification; no stable-runtime claim is made here yet.

The first whole Mac run failed the recovered-delay fixture under concurrent
240 fps synthetic workloads. The fixture now uses30 fps, records observed
cadence misses/path counters on failure, and isolates the timing scenarios.
Internal producer/reader/ACK concurrency and all admission/recovery assertions
remain. Restoring the old cumulative-event signal in the30 fps fixture still
fails (4 Mbit/s becomes1.96 Mbit/s), while the new signal passes individually.
The parallel-fixture failure is retained; isolated/full requalification follows.

Temporary metadata instrumentation localized the remaining fixture reduction:
with no late receipt, no failures, no misses and fresh successful delivery,
a path RTT change from1.465 to2.239 ms cut the rate again. Integer-millisecond
rounding and the relative-only growth threshold classified harmless low-RTT
noise as congestion. The controller now also requires at least10 ms absolute
growth. Existing loss/congestion-event and material RTT-growth reactions remain;
QUIC's underlying control is unchanged. A regression alternates1/2/3 ms clean
samples and then introduces a40 ms sample to verify both behaviors. Temporary
instrumentation was removed and is never part of the deployed binaries.

Current production revision passes whole all-feature workspace suites:
Mac784 passed,0 failed,2 ignored; Linux786 passed,0 failed,11 ignored;
107 result groups each. Strict all-feature/all-target workspace clippy and
formatting pass both. Full desktop regressions include the real recovered-delay
and sustained-blockage cases, alongside resource/lifecycle/impairment checks.
The RTT-noise regression fails with the relative-only guard (461888 bps instead
of8 Mbit/s) and passes with the absolute margin, including a material-growth cut.
All failed fixture observations and temporary diagnostic receipts are retained.
The current code is installed on both devices with backed-up atomic replacements;
a fresh native mixed-workload qualification is running. Prior failed longer-run
observations are not relabeled as successes.
