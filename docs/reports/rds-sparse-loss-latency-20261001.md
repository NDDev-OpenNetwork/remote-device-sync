# Sparse desktop loss and input latency — 2026-10-01

## Defect and regression

A direct WAN trace on the previously installed implementation recorded 44
bitrate reductions over three minutes. Thirty-three followed an individual
lost packet; there were no producer deadline misses. The selected path ended
with 38 lost packets among 3,227 sent datagrams, while the target fell from
4 Mbps to approximately 114 kbps. Tiny 250 ms packet samples and a separate
congestion-event penalty exaggerated ordinary sparse loss. A native color
response probe reached 1,122 ms and then failed to observe its eleventh click
within five seconds. Recent receive/presentation progress was insufficient
proof of responsive interaction.

The deterministic sparse-feedback regression uses five datagrams per pacing
observation, one lost packet per 100, and successful media acknowledgements.
It fails on the old controller: 4 Mbps becomes 273,686 bps after 80 seconds.
The corrected controller reaches its existing 8 Mbps ceiling. Separate tests
retain severe short-burst loss, sustained RTT growth, true blocked delivery,
correlated receipt/path cooldown, path changes and frame reference ordering.

## Change and diagnostic limits

The implementation follows the sample, RTT-confirmation and cooldown bounds
in [the native contract](../native-viewer.md#sparse-traffic-and-latency-diagnostics).
QUIC packet pacing/retransmission and bounded media acknowledgement failure
recovery remain active. No wire layout, credential or authorization policy
changes. The successive runtime increments are `8d13c53`/`c1fe3ad` (loss and
input timing), `c7db292` (modifiers/surface diagnostics), `a88dab4` (actual
XKB physical mapping and blocked-receipt episodes), and `5fd1107` (accepted
input wakes idle capture). `2f12408` requests native application activation
on explicit launch, and `8d8af90` preserves compressed references through local
IPC backpressure. Subsequent fixture/CI edits do not change runtime code.

Private logs record a bounded reason and path/media/producer metadata for each
bitrate reduction. Independent five-second records include path counters and
successful input-injection duration. Native input ACK and UI queue histories
are local-clock measurements, bounded to 128 pending sequences and 1,024
samples; reconnect clears pending observations. They contain no key/button
values, screen or clipboard contents, endpoint identity or address. ACK
completion and GPU submission are not physical display measurements.

## Native keyboard correction

The wire uses evdev physical codes, but a legacy XFree86 keyboard can place
Up at native code 98 and Print Screen at 111. The old `evdev + 8` assumption
therefore injected Print Screen for Up. The server now resolves actual XKB
physical names; releases retain the native key recorded at press. Linux unit
fixtures cover both maps and ambiguous/missing names. Disposable Xvfb runs
qualify Alt+Up on both the default map and an explicit legacy `us,ru` map;
Linux CI retains the second case. Installed native Mac Option/Alt+Up, Left,
Right and Down reached the isolated target fixture as the correct arrows with
Alt set. This evidence covers those chords, not all OS-reserved shortcuts or
IME/composition.

## Input and large-frame capture

A contiguous blocked receipt now produces one soft reduction. Repeated cuts
while admission pauses capture cannot shrink the already encoded keyframe;
the old four-second regression fell to 960,400 bps, while the corrected
controller retains its first 2.8 Mbps reduction. Progress resets the episode
and distinct hard failures still react. Frame/read/ACK budgets stay bounded.

Accepted input also opens a 250 ms capture burst at existing cadence/admission
limits, without requesting an IDR. A real-UDP fixture with an idle capture
source fails before the change because accepted input produces no frame within
300 ms. It passes with the wake hint, receives the next delta in order, and
verifies that view-only input cannot wake capture. Installed composited-desktop
response remains a separate measurement; a watched root DAMAGE stream alone
must not be assumed to describe redirected application repaint.

## Validation and remaining qualification

The final runtime increment `8d8af90` passes the complete workspace lane:
795 cases on macOS (two platform ignores) and 800 on Linux (12 platform
ignores), across 109 groups on each platform. Strict all-targets/all-features
clippy and native release builds pass on both machines. Dependency/format
checks and both native CI builds passed for that runtime source. Its macOS CI
workspace run found a separate asynchronous-close race in the owned transport
telemetry fixture: selection of a successor was observed before the engine
processed closing the initial path. `f990012` waits for both conditions and
retains the single-live-path assertion; its focused all-features regression
and strict clippy pass. The failed artifact is retained, and fresh CI outcomes
are tracked independently of these local results.

Current installed native latency and a sustained mixed workload remain
required before claiming full stability. Earlier full-workspace attempts hit
a 256-descriptor test-process limit; qualification uses a per-command limit
of 4,096. An unrelated two-second owned relay fixture timed out in a prior
macOS CI attempt; its failed artifact and isolated passing repeat are retained.
No milestone gate is closed by this report.

## Local encoded relay backpressure

A real H.264/real-UDP regression pauses the encoded consumer for 200 ms.
With the old four-item newest-wins queue, the next received frame is sequence
5 instead of 1, despite a continuous delta reference chain. The corrected
one-item FIFO backpressures the relay receiver and delivers all eight deltas
in order; all eight decode successfully. Decoded newest-frame presentation
remains unchanged. Encoded-mode admission respects the existing four-reader
budget by leaving further streams in the bounded route inbox while busy;
raw-mode excess refusal, body bounds and read deadlines remain unchanged.
The ordered receiver also retains a just-completed missing reference together
with its three admitted successors, within the same four-reader limit.
Existing WAN reference assertions/timeouts remain and the relay fixture now
consumes the encoded tap explicitly alongside metadata.

The final runtime binaries are installed on the existing Mac client/agent and
Linux serving agent, preserving identities, policy and the existing desktop
seat. The same native window recovered in 1,375 ms after the coordinated
server binary restart. This is a lifecycle observation, not a latency or soak
qualification. Separate extended recovery coincided with repeated local
network-unreachable errors over approximately 26 minutes; it is retained as
failed continuity rather than presented as successful stability. Native input
ACK tail samples also remain materially above the network median during some
interaction bursts. Current application-to-pixel latency is unqualified:
OS-covered/off-display windows and unavailable ScreenCaptureKit capture do not
prove a visible response. No input is posted when the target visibility guard
fails. Physical display, current-build mixed-load continuity and quality gates
remain open.
