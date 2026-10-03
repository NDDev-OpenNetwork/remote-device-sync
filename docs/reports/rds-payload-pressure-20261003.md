# Payload-qualified soft delivery pressure — 2026-10-03

W6.6/W6.7 follow-up. This is a controller regression receipt, not closure of
native click-to-visible, loss/jitter, long idle or quality acceptance.

The prior soft-delay classifier could cut offered bitrate repeatedly when
small desktop updates crossed their receipt deadline despite continuing
successful delivery. It considered receipt counts only. The reproduced fixture
has 240 pacing observations, clean stable path samples, successful receipts in
every observation and a newly delayed small update each time. Before the change,
the encoder target fell below its initial 4 Mbps; the new quality assertion
failed. The initial failure is retained as local development evidence.

The receipt producer now counts delayed payload bytes when crossing the soft
deadline, and pacing reads their interval delta. Timing-only pressure requires
16 KiB in each of two consecutive 250 ms observations. The small-update fixture
now records actual 1024-byte delayed updates through the same feedback collector
used by receipt workers. It preserves the target instead of treating sparse
packet delay as encoder overload. Substantial delayed payload still reduces
load with successful receipts, and one small interval rearms that sequence.

The threshold is a conservative application heuristic; it does not prove the
cause of delay or estimate network capacity. Separate stalled-receipt and hard
failure paths remain, as do path loss/RTT adaptation, delivery holds, bounded
growth, frame admission/deadlines and reference-preserving encoder updates.
The cumulative `delayed` counter denotes first deadline crossings, not completed
delayed frames. Private health/reduction logs expose counts and bytes without
payload, input values, peer identities or clipboard content.

Verification on macOS arm64: all 45 session unit tests passed; strict workspace
all-target desktop-feature clippy passed; complete desktop-feature unit and
integration tests passed with `x11,viewer` (112 tests, no failures). Linux/X11
display cases were not run on macOS; the native Linux lane owns them.
Before/after fixture evidence covers the
controller and production feedback accounting, without claiming physical
network or native visibility performance. Fresh Linux/macOS CI and installed
serving/receiver qualification belong to the exact published candidate.
