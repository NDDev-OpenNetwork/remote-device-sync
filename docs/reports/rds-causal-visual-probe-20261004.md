# Controlled native visual response measurement — 2026-10-04

W6.8 qualification increment. Existing RTT/ACK and decode-to-submit statistics
do not identify which frame contains an input's application response. An opt-in
controlled marker now binds native left presses in a known source-pixel target
to a cumulative response visible in an actually submitted GPU frame.

The viewer also retains up to ten private metadata incident windows: thirty
two-second snapshots before a trigger and ten seconds afterward. Pending input
ACKs at 250 ms, video inactivity at three seconds, visible submission inactivity
at one second and reconnect counter changes trigger recording with a cooldown.
Completed slow ACK counter changes also trigger recording when their entire
delay fell between periodic snapshots.
Graceful shutdown saves an incomplete window. These are diagnostic thresholds,
not latency guarantees. Sub-two-second faults can fall between snapshots; slow
completed ACKs also emit rate-limited warnings at the default log level.

Reports expose current input queue depth, canceled pending ACK count, slow ACK
count, last ACK latency and local attempt epoch. Server capture, pacing, writer
and receipt workers retain the owning tracing span. Opt-in timing traces record
production/decode and input injection/ACK-write durations without key codes,
clipboard, pixel or application text. Session IDs are local process correlation
IDs; timestamps on different hosts must not be subtracted as network latency.

The diagnostic subscriber previously excluded INFO connection spans under a
WARN filter. Its per-layer `event_scope` then emitted diagnostics with no
session ID even while operational events retained it. A negative regression
reproduced `session_id: null` on the preceding implementation; retaining numeric
connection spans in the diagnostic layer fixes it without enabling filtered
INFO messages or duplicating operational records. All observe tests and strict
clippy pass after the correction.

Managed native delivery preserves the encoded sequence in the exact pending
raw frame. A matching visual response logs both input and frame sequence, so it
can be joined to production, pacing, transport and decode traces. The GUI thread
uses the retained attempt span rather than inheriting an unrelated event-loop
context. The legacy untagged/direct raw-frame API remains available and reports
no sequence instead of guessing from an independently consumed header mailbox.

The macOS workspace CI also exposed a pre-existing Noq churn-test assumption:
it expected exactly one live path after closing the original path, although
discovery may establish additional validated paths. The fixture now records
every explicitly retired ID and requires all to be absent while retaining the
high-ID successor, coverage and sticky-lag assertions. The original failure
was not reproduced in a serial local run; the corrected contract passed three
local repetitions and strict Noq clippy. CI acceptance remains required.

The next macOS CI failure reported a recovered-delivery bitrate cut with zero
producer misses, zero packet losses and only 24922 total sent bytes. A separate
negative controller regression reproduced repeated sparse RTT changes driving
healthy acknowledged updates from 4 Mbps to about 102 kbps. The controller now
requires at least 32 KiB offered across its two elevated RTT observations and
new transmitted bytes in the confirmation observation. Re-reading one cached
snapshot cannot confirm a cut. This is an application load heuristic, not an
estimate of available link capacity; transport congestion control, sampled loss,
producer deadline and actual delivery pressure retain their responses.
Both new controller regressions, the full expanded desktop unit suite and three
repetitions of real recovered/sustained delivery-pressure tests passed locally.

The remote controlled target owns a known 48-bit marker prefix and increments a
16-bit counter once per button press. The viewer samples only that descriptor's
64 cells, rejects ambiguous/wrong-prefix/truncated data and records counts and
times. Bounds prevent out-of-frame indexing; reconnect/rollback cancels samples.
Queue/sample limits and missing-marker/eviction counters make failed or incomplete
qualification visible. The ordinary viewer path has no marker sampling enabled.

Unit tests cover unrelated/ambiguous frames, malformed geometry/data, outside
clicks, cumulative rapid responses, duplicate presentation, canceled reconnect,
occluded/unavailable surfaces, lost marker anchors and completed short ACK stalls,
samples and actual H.264 marker encode/decode. These do not establish physical
network latency. Native installed qualification follows separately, without
equating GPU submission with optical scanout or retaining desktop content.
