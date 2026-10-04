# Controlled native visual response measurement — 2026-10-04

W6.8 qualification increment. Existing RTT/ACK and decode-to-submit statistics
do not identify which frame contains an input's application response. An opt-in
controlled marker now binds native left presses in a known source-pixel target
to a cumulative response visible in an actually submitted GPU frame.

The viewer also retains up to ten private metadata incident windows: thirty
two-second snapshots before a trigger and ten seconds afterward. Pending input
ACKs at 250 ms, video inactivity at three seconds, visible submission inactivity
at one second and reconnect counter changes trigger recording with a cooldown.
Graceful shutdown saves an incomplete window. These are diagnostic thresholds,
not latency guarantees. Sub-two-second faults can fall between snapshots; slow
completed ACKs also emit rate-limited warnings at the default log level.

Reports expose current input queue depth, canceled pending ACK count, slow ACK
count, last ACK latency and local attempt epoch. Server capture, pacing, writer
and receipt workers retain the owning tracing span. Opt-in timing traces record
production/decode and input injection/ACK-write durations without key codes,
clipboard, pixel or application text. Session IDs are local process correlation
IDs; timestamps on different hosts must not be subtracted as network latency.

The remote controlled target owns a known 48-bit marker prefix and increments a
16-bit counter once per button press. The viewer samples only that descriptor's
64 cells, rejects ambiguous/wrong-prefix/truncated data and records counts and
times. Bounds prevent out-of-frame indexing; reconnect/rollback cancels samples.
Queue/sample limits and missing-marker/eviction counters make failed or incomplete
qualification visible. The ordinary viewer path has no marker sampling enabled.

Unit tests cover unrelated/ambiguous frames, malformed geometry/data, outside
clicks, cumulative rapid responses, duplicate presentation, canceled reconnect
samples and actual H.264 marker encode/decode. These do not establish physical
network latency. Native installed qualification follows separately, without
equating GPU submission with optical scanout or retaining desktop content.
