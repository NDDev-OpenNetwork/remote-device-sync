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
