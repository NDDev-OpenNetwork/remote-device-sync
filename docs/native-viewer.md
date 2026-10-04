# Native desktop viewer

Automatic diagnostics also save `incident-<timestamp>-<pid>.json` windows in the
private viewer log directory. Up to ten windows survive ordinary log rotation;
each is capped at 256 KiB. They contain at most thirty prior snapshots and six
following samples, metadata only. The recorder warns on a pending ACK beyond
250 ms, three-second decoded-video inactivity, visible submission inactivity
beyond one second or a reconnect. It saves ten seconds after the trigger and
uses a thirty-second cooldown. Occlusion alone does not trigger a surface fault.
Graceful shutdown preserves an interrupted window; abrupt process death can
leave only the regular live snapshot and logs. Two-second sampling can miss
short pending incidents, so completed slow-ACK counter changes also trigger a
window, alongside independent rate-limited warnings.

`session_epoch`, `input_queue_depth`, `input_acks_canceled`, `slow_input_acks`
and `last_input_ack_ms` complement the existing bounded latency percentiles.
Percentiles summarize the retained sample buffer, not a timed rolling interval.

## Controlled causal visual diagnostics

The optional `--diagnostic-visual-probe FILE` accepts a strict, regular JSON
descriptor capped at 4 KiB. It selects a known 64-cell black/white marker, a
nonzero 48-bit prefix, a 16-bit cumulative response counter and the remote
source-pixel click rectangle. The controlled application must increment the
counter once per left-button press; no unrelated actor may drive that target
during qualification. The descriptor is an explicit diagnostic, not a normal
desktop setting or remote protocol extension.

Only native left presses inside that rectangle begin a sample, after a matching
marker has already been presented. The exact pending BGRA frame must reach
`DrawOutcome::Presented` with the expected marker/counter before completing it.
ACKs and decoded-but-unpresented images cannot finish the measurement. Reconnect,
counter rollback, unavailable/occluded surfaces or a missing marker cancel
pending samples and require a fresh presented anchor. At most 128 clicks and 1024 latency
samples are retained; replaced intermediate frames can expose cumulative results.
Reports contain timing/counts, missing-marker and cancellation/eviction counters,
without retaining pixels or input text. Marker detection is tested through actual
H.264 encoding/decoding in the codec lane.

Managed delivery attaches the exact encoded frame sequence to its pending raw
frame. Visual response and submission traces carry it alongside the local
attempt context, allowing stage correlation. Untagged/direct raw delivery
retains a missing sequence; it does not pair unrelated header/frame mailboxes.

`input_to_submit_*` measures the software native input-to-GPU submission boundary
on one local monotonic clock. It includes the input queue, network, native remote
application response, capture/codec and local rendering. It excludes automation
delay before the native event and does not claim compositor scanout or optical
glass-to-glass timing. Source resolution, marker geometry, occlusion and input
delivery must be verified on the installed devices before accepting a result.

The `desktop` CLI feature builds a native winit window with a wgpu surface:
Metal on macOS and supported native GPU backends on Linux. Software H.264
decoding stays on bounded blocking workers. The serving device needs a real
capture/input backend; the current Linux implementation uses X11/XTEST.
macOS capture, VideoToolbox and Wayland serving remain separate work.

Managed native dispatch and acknowledgement observation use separate retained
async legs from ordered receive/decode. The agent's separated local extension
keeps video and upward controls on the first same-UID Unix socket and transfers
input ACKs, heartbeat echoes and clipboard-ready metadata over a second socket.
A blocked video-body write or a busy decoder cannot hold event observation.
`managed_events_separated` in the viewer report identifies this installed mode.

Each control write remains bounded to two seconds; codec calls retain their
five-second caller bound and global worker limits. After three and eight seconds
without decoded progress, the watchdog requests an independent recovery image on
the existing control channel. Only a decoded frame rearms these two attempts;
heartbeat echoes do not. Fifteen seconds without progress still ends the desktop
and triggers bounded reconnect, so a responding control channel cannot mask a
nonfunctional video session.
Decoder state, FIFO encoded ordering and the one-message media queue remain.
Either socket's departure ends this desktop by EOF, with both owned reader tasks
aborted; no frame is appended after canceling a potentially partial write.
Shared TCP/sync connections and other desktops are not closed.

Legacy managed/headless APIs retain their original combined socket. The native
viewer requires the additive v5 separated-channel commands; an older agent
refuses them without silently reverting to the coupled route. Update the local
agent and viewer together. Remote desktop framing and grants are unchanged.
See [the independent-event qualification](reports/rds-event-isolation-20261002.md).

The viewer uses ordinary OS window stacking and can move behind other
applications. An occluded Metal surface may pause presentation. Returning focus
or uncovering the window requests an immediate redraw of its latest image.
The application icon is shared by the macOS bundle, native viewer and Linux
launcher; its source and reproducible derivatives live in the desktop assets.

```sh
cargo build --release -p rds-cli --features desktop
rds desktop <ticket-or-device> --max-fps 60
rds session desktop --session <session-id>
rds-viewer <ticket-or-device>
```

These commands reuse the local agent's endpoint and same-UID session manager.
The operator also runs `rds-agent`. `rds desktop --direct` explicitly uses its
separate configured endpoint; no implicit direct fallback is introduced.
`--headless` keeps decode/statistics operation without creating a window.

The window scales video without changing its aspect ratio and translates
pointer positions into the original display coordinates, including when video
is downscaled. Physical keys use the wire's evdev vocabulary; the target's
keyboard layout interprets them. IME/text composition, rich clipboard/audio and
monitor-switching UI are not implemented. Focus loss releases held keys and
buttons. Input queue overflow closes the session instead of silently losing
a release. The queue holds at most 1024 controls. Only adjacent absolute pointer
moves on the same display collapse; motion before a button, key or scroll remains
a position barrier. Full-queue rejection leaves the accepted prefix intact and
records overflow before closing. It never evicts an earlier click's position.

Only one pending decoded frame is retained for presentation. The GPU surface
requests `AutoNoVsync` and one frame of latency, with the backend's supported
fallback. Submission timing does not prove when a physical pixel becomes
visible. Completed encoded frames are briefly ordered before decode so a delta
that finishes first cannot discard its in-flight reference keyframe. Ordering
retains at most three successor bodies. The missing-reference timer uses two measured control RTTs, bounded to
100–1000 ms (250 ms before measurement). It requests IDR recovery only after
admitted readers finish or reach their own bounded deadlines. Heartbeat RTT
uses bounded local send/echo correlations; caller timestamps stay opaque, so
reconnecting a managed desktop does not compare two different clock origins. Jitter does not discard successors of a reference still
being received. The existing global encoded/decode limits still apply.

The sender also measures frame delivery receipts independently of QUIC packet
loss counters, which may look clean while a reliable relay queues traffic.
A receipt delayed beyond three sampled path RTTs (bounded to 250–1000 ms) is
tracked until it completes or its owned task is canceled. The cumulative delayed
count increases when a receipt first crosses that deadline, not when it completes;
currently outstanding late receipts are tracked separately. Timing-only bitrate
pressure requires two adjacent 250 ms observations, each with at least 16 KiB of
newly delayed payload. Sparse small updates can be late from retransmission or
jitter without the encoder exhausting link capacity; repeatedly cutting their
encoder target can instead degrade quality and increase codec skips. An isolated
substantial delay cannot trigger this soft path. Outstanding blocked delivery
without successful receipts and hard failures retain their independent response.
These signals estimate delivery pressure without proving its physical cause.
A hard delivery failure still reduces
load immediately. A five-second recovery hold and growth of at most 1% per
sample with fresh receipts prevent immediate return to a sustained backlog.
Media reductions coalesce over one second; simultaneous path/media observations
apply the stronger response once. Existing bitrate bounds, frame deadlines,
three-frame admission and reference-preserving live encoder updates still apply.
Health and reduction logs expose newly delayed payload bytes alongside frame
counts. The 16 KiB qualification is a bounded application heuristic, not a
measurement of path capacity or a minimum image-quality guarantee. See
[the regression receipt](reports/rds-payload-pressure-20261003.md).

Loss-only reductions now also respect recent timely transport goodput. Two
adjacent one-second windows must each contain at least three complete timely
frame receipts and 4096 payload bytes; the lower window's rate supplies a
conservative floor with 20% headroom. Late, obsolete and failed frames supply
no such credit. Unknown/changed paths, outstanding late receipts, actual failure
or stale/idle evidence clear the observation. RTT/producer pressure, delivery
holds and negotiated ceilings retain their existing limits. This estimates
confirmed transport delivery, not decoded/displayed quality or total capacity.
See [the qualification boundary](reports/rds-confirmed-goodput-20261002.md).

With no unconfirmed media, an independent key of at most 64 KiB starts under
QUIC pacing without an extra application wait. Larger keys, dependent frames
and nonempty media retain pacing and the same half-second debt bound. The
existing key-receipt capture barrier remains; waits of at least 100 ms are
logged with frame metadata, never pixels.
Sender health includes delayed-delivery counts and latest receipt duration.
Independent production health continues during a stopped video writer and
records production activity, intentional codec skips, latest produced-frame
age, admission/keyframe waits and path RTT;
diagnostics contain frame metadata, never pixels or clipboard contents.

The viewer reopens an interrupted desktop channel with a fresh session route.
When the managed connection disappeared, it reconnects the same pinned peer
through the manager. Explicit ticket address hints are retained across retries;
a verified device name freezes to its authenticated identity. A pinned ticket
cannot silently switch identities. Reconnect backoff is bounded to eight seconds; inputs
queued while disconnected are discarded. No TCP/SSH exec or file operation is
replayed by this recovery. A grant is reused only while the agent accepts it;
automatic grant issuance/renewal remains GDS work. Fifteen seconds without a
decoded frame triggers a new desktop session after the two bounded repair
attempts. Closing the window cancels and
joins its network worker, leaving unrelated managed streams intact.

Each reconnect warning records the last network/render stage, decoded and
submitted frame ages, UI dispatch age, outstanding input age/count, control RTT
and occlusion. These metadata explain the interrupted state without logging
keys, typed text, pointer coordinates, clipboard contents or screen pixels.

Discarding input during a reconnect pause keeps the original retry deadline;
pointer/key activity can neither shorten nor restart it. Window close and
cancellation still interrupt the pause immediately.

Both halves of desktop control streams use the shared highest QUIC priority.
Ping, Info and authorization/renewal requests and replies use the same class,
above media frames. TCP/sync bodies keep their existing priority. This local
stream scheduling cannot overtake datagrams already emitted or bypass congestion
and flow-control limits; complete mixed-load/renewal acceptance remains separate.

The separate `rds-viewer` accepts `--control-dir` and `--grant-file`. When run
without a target, it uses the selected managed session or an optional private
`viewer.json` beside the endpoint key:

```json
{"schema_version":1,"target":"device-a","display":0,"max_fps":60}
```

An optional `grant_file` names a bounded local signed-grant file. Explicit
command-line values override the configuration. This file contains deployment
choices and is not shipped with an application bundle.

Build a new local macOS application:

```sh
python3 scripts/package-viewer.py --binary target/release/rds-viewer \
  --output "$HOME/Applications/RDS.app"
```

The package contains the native executable, generic Info.plist and the shared
ICNS application icon. Dock and Finder use the bundle icon; direct CLI launches
set the same application icon through the native AppKit adapter. Its local
ad-hoc signature is not Developer ID signing or notarization. The engineering
preview release archives retain their documented binary contents; adding a
local viewer does not rewrite an existing published archive.

For a bounded run, use `--duration 30 --report <new-file.json>`. Reports contain
received/submitted/replaced frames, first/last frame timing, video dimensions,
receive-to-GPU-submission p50/p95, control RTT, input ACK count and reconnect
count/recovery time. Redraw/surface-skip counts distinguish a covered window
from presentation progress. Managed sessions also report capture/encode and
encode-to-send p50/p95 from the serving endpoint's monotonic timestamps;
these durations do not include network transit or physical display delay.
No screenshot, peer key, address, clipboard contents or credentials are
written to this report. For actual input-to-visible measurement use a target
test window with a known color change and observe the rendered result; do not
present the local receive-to-submit metric as full end-to-end latency.

`RDS_DESKTOP_OUTPUT_HEIGHT` on the X11 serving agent selects a bounded software
downscale (16–4320 pixels, no upscaling, even codec dimensions). Omitting it
retains native resolution. Original display capabilities and input coordinates
remain unchanged. This reduces capture conversion/codec/network cost at the
explicit resolution tradeoff. Capture timestamps precede capture/conversion;
damage readiness wakes via the X socket rather than a fixed 25 ms poll delay.
An idle wake starts capture immediately without counting skipped idle slots
or sub-slot scheduling jitter as encoder starvation. This prevents an idle
desktop from reducing its bitrate solely because it resumed after waiting.
The software producer treats `max_fps` as a ceiling and accounts for measured
capture/conversion/codec work when scheduling its next frame. Fixed capture
CPU cost cannot reduce network bitrate simply by exceeding a 60 FPS interval.
Session admission enforces that capture-start ceiling independently of damage
readiness, including an immediately ready platform or injected producer. Time
spent capturing/encoding counts toward the interval; cancellation is checked
while waiting. This also bounds low-FPS diagnostic sessions during active damage.
`RUST_LOG=rds_desktop=debug` reports bitrate changes and deadline-miss counts;
the `trace` level adds frame sizes and sender stage durations, without pixels.

For a Linux desktop launcher after installing `rds-viewer` on PATH:

```sh
install -Dm644 crates/rds-desktop/assets/app-icon-512.png \
  "$HOME/.local/share/icons/hicolor/512x512/apps/org.nddev.opennetwork.rds.png"
install -Dm644 crates/rds-desktop/assets/org.nddev.opennetwork.rds.desktop \
  "$HOME/.local/share/applications/org.nddev.opennetwork.rds.desktop"
```

Native Linux windows also carry the same embedded icon. Launcher installation
contains no endpoint identity; the normal private viewer configuration applies.

## Quality selection

The macOS application presents a native quality chooser before opening its
remote session. Full HD (1080p) is the default; HD (720p) and Original are also
available. `--resolution full-hd|hd|native` skips the chooser for explicit CLI
or automated launches. Output preserves aspect ratio and does not upscale a
smaller source. This is encoded video geometry, not the local window's size.

The additive `DesktopV3` greeting and `DesktopProfile` manager command carry the
requested height before capture starts. They preserve existing wire tags and
legacy greetings; older peers refuse the extension without a silent fallback.
Install the current viewer and both agents together. An explicit session choice
overrides `RDS_DESKTOP_OUTPUT_HEIGHT`; legacy sessions keep that deployment
fallback. Zero selects original geometry, otherwise the bound is 16–4320 pixels.

## Explicit text paste

On macOS, copying text in a local app with Cmd+C and pressing Ctrl+V inside RDS
reads the native NSPasteboard for that paste gesture. Text transfers on the
ordered control channel before V reaches the remote application. The X11 agent
owns the CLIPBOARD selection and serves UTF8_STRING/TARGETS/TIMESTAMP and ICCCM
INCR for larger data, without a clipboard helper process. This is real clipboard
publication, not typing text through keyboard-layout substitutions.

One transfer is bounded to 1 MiB of UTF-8, with 32 KiB control chunks, exact
ordered offsets and a five-second assembly deadline. There are at most four
active native selection workers and eight outstanding INCR requests per worker.
View-only sessions refuse publication. Publication failure ends the control
session before subsequent paste input can consume an unrelated old clipboard.
Contents are neither logged nor written to disk. There is no background scan or
automatic export of every local clipboard change. Images, files, rich formats,
reverse clipboard and macOS Cmd+V translation remain outside this text path.

## Persistent viewer diagnostics

`rds-viewer` automatically writes private logs under `viewer-logs` beside the
endpoint configuration (normally `~/.config/remote-device-sync/viewer-logs`).
Each log part is 8 MiB maximum; ten generated parts are retained. Files are 0600
in a validated 0700 directory without symlink components. The existing bounded
telemetry output adapter performs logging away from UI/network threads.

Every two seconds a small `state-<pid>.json` snapshot is atomically replaced.
It contains UI dispatch age, encoded/decoded/presented-frame ages, network/render
stage, occlusion, pending CPU bytes, received/submitted/replaced frames, actual
GPU upload count, reconnects, watchdog repair requests and stage latency. Snapshots contain no image,
clipboard, peer or credentials. Stalled input/heartbeat writes and decode waits
are bounded separately; reconnect causes and panics are recorded. `--report`
still emits an end-of-run receipt, while the live snapshot survives a hung or
terminated UI. CLI desktop commands may opt in with `--diagnostics-dir` pointing
to an existing private directory. Independent UI probes share the bounded wake
flag; a stalled UI cannot create an unbounded event queue.

`control_echo_age_ms` measures time since an actually observed heartbeat echo,
using the viewer's clock. The last RTT is a historic sample; it does not prove
that control traffic still flows. Reconnect warnings include echo age, and
managed stages distinguish session lookup, peer connection, identity verification
and desktop opening so a failed peer dial is not mistaken for a video-only stall.

Surface acquisition now precedes any GPU upload. An occluded surface retains
only the newest CPU image, rather than queuing staging buffers without submission.
Managed IPC framing is read by one owned bounded worker; canceling `recv()` in a
select never loses partially consumed message bytes. Drop aborts that worker.

The software encoder uses actual monotonic timestamps with OpenH264's timestamp
rate control. Idle waits and slower real capture can replenish the bitrate budget;
no constant-zero timestamp fabricates a fixed 60 FPS clock. An intentionally
skipped codec frame preserves the reference sequence and does not force an IDR.
Capture/encode errors and dropped encoded references still require recovery.
Bitrate changes apply typed native OpenH264 target/max options after encoder
initialization, preserving references rather than producing an adaptation IDR.
RTT adaptation detects new increases between valid samples with at least
10 ms absolute growth, avoiding relative-only penalties from 1–3 ms noise; an unchanged high
RTT does not repeatedly penalize the stream against a permanent startup value.
Incomplete delta bodies release reader permits after three seconds; a validated
independent keyframe has eight seconds including its header. Recovery frames
often require more initial congestion-window rounds than a delta. Missing
headers still expire after three seconds; handshake/control deadlines remain
separate. Receiver health logs
include admission, completion, rejection, timeout, gap and keyframe counters.
Timeout records include frame sequence and partial media byte count, without
logging the payload, so stalled headers and stalled bodies can be distinguished.

The sender retains at most three frame streams awaiting transport delivery
confirmation, with five-second delta/ten-second keyframe bounds and
reset-on-cancellation ownership. A
queued FIN alone is not a delivery receipt. This leaves capacity below the
receiver's four-reader budget; capture pauses before encoding when its two-slot
queue is full. Sender health distinguishes queued, acknowledged and unconfirmed
frames. Transport acknowledgement is not proof of decode or presentation.
Capture also waits for an outstanding keyframe's acknowledgement before encoding
successors. This prevents later deltas from timing out while a large recovery
keyframe is still transferring, and prevents duplicate IDRs from competing on
slow links. Capture obtains one of three owned media permits before acquiring
pixels; queued, currently written and unacknowledged frames share that budget.
Permits release on acknowledgement, rejection or cancellation. The bounded
writer preserves queued references in order and exposes total pending work.
Admission pauses rebase producer cadence; deliberate network waiting is excluded
from encoder-starvation signals, so it cannot repeatedly reduce image quality.
The software encoder disables periodic IDRs. New sessions, geometry changes and
explicit recovery still produce independent frames; the prioritized reliable
control stream carries resync requests and the silence watchdog reopens a stuck
session. A valid reference chain no longer pays a large recovery transfer every
240 frames. Automatic scene-change decisions remain the codec's responsibility.

Text paste sends 16 KiB chunks while accepting the existing 32 KiB wire bound.
Reassembly permits at most five seconds without progress and thirty seconds in
total. A progressing large transfer is no longer rejected solely because its
first chunk arrived more than five seconds earlier. Errors distinguish bounds,
identity/offset changes, idle/total deadlines and invalid UTF-8 without content.

## Sparse traffic and latency diagnostics

The serving bitrate controller aggregates at least 100 sent datagrams before
classifying packet loss. Loss above 2% and up to 10% holds the path estimate;
it does not repeatedly cut successfully delivered media. Fresh media receipts
permit recovery probes capped at 1% per pacing step, within the negotiated
ceiling and after the existing delivery hold. No receipt means no growth.
Loss above 10% reduces the estimate; a short burst losing at least five packets
and more than 10% can react earlier. Individual transport congestion events remain
diagnostic: QUIC still handles retransmission and packet pacing. A large RTT
rise needs two consecutive 250 ms observations; path and media penalties for
one burst share a one-second cooldown. Actual failed media receipts and
sustained blocked delivery retain their bounded recovery and bitrate response.
This prevents sparse losses and continuing moderate loss with timely receipts
from repeatedly collapsing Full HD quality. It does not establish a quality or
latency guarantee on a particular network.

Private normal-level server logs now retain every bitrate reduction's path
counters, delivery state and producer misses. Five-second health records also
include direct/relay selection and the maximum successful input-injection
duration in that interval. No input values or content are recorded. Viewer
reports correlate up to 1024 dispatched input sequences using only the viewer's
local clock, with bounded 1024-sample histories. They separately count dispatch
attempts, received/matched/unmatched ACKs and evicted tracking records. Dispatch
is observed before the bounded write and does not prove remote injection; an
unmatched ACK supplies no latency sample. Input ACK p50/p95 includes local
queueing, transport, server injection and reply handling; queue p95 and the
oldest pending ACK age distinguish an input backlog from stale video. These
ACK measurements do not establish that the target application changed pixels.

## Keyboard modifiers across platforms

The Mac viewer explicitly treats both Option keys as remote Alt. Native
modifier flags reconcile the held remote keys before the next key or pointer
event, including when a modifier was already held as the viewer gained focus
or the OS omitted a separate modifier-key event. Known right-side keys stay
right-side keys; flags with no known side default to the left modifier.
Focus loss still releases held keys and buttons. Ctrl remains Ctrl, Shift
remains Shift, and Command maps to the remote Super/Windows modifier; physical
keyboard layout and text interpretation stay with the remote desktop.

For Ubuntu shortcuts, use Option+arrow for Alt+arrow, Control for Ctrl and
Command for Super. This does not turn local OS-reserved shortcuts into remote
input, or provide the still-planned IME/composition integration.

Explicit viewer launches request window focus once. Redraws and reconnects
retain normal window stacking and never reactivate the application. Surface
occlusion, acquisition timeout, reconfiguration and absence of a picture have
separate diagnostic stage names, so a covered window is not confused with a
presentation failure.

A contiguous blocked media receipt produces one soft bitrate reduction.
Capture admission can already be paused while a large keyframe is in flight;
repeatedly lowering future encoding rates cannot shrink that payload. Fresh
acknowledgements or completion reset the episode; distinct hard failures still
react immediately. This prevents startup/recovery keys from driving otherwise
healthy sessions to the bitrate floor merely by taking several RTTs to arrive.

Accepted input opens a bounded 250 ms capture burst without requesting a
keyframe. The X11 producer checks this session-clock deadline while waiting
for damage and captures at its existing cadence/admission limits during the
burst. This avoids a one-second idle refresh wait if a redirected/composited
application repaint is not represented by the watched root DAMAGE stream.
Idle capture still pauses after the deadline. View-only or rejected input
cannot open a burst; this is an interaction hint, not proof of an application's
response or a replacement for end-to-end latency qualification.

On macOS, explicit startup requests application activation after creating the
window, in addition to window focus. The modern AppKit activation request is
used when the runtime supports it; older supported systems use their legacy
activation API. AppKit may decline an activation request, and covered-window
snapshots still cannot establish physical presentation. Redraw/reconnect does
not request activation or raise window level.

The local encoded relay tap holds at most one queued compressed frame and
backpressures its wire receiver while IPC/decoding is busy. It preserves H.264
reference order: dropping compressed predecessors used to force unnecessary
IDR recovery during local stalls. Newest-frame replacement remains valid only
after decode. Closing the encoded receiver wakes its blocked sender and ends
that receive leg; session cancellation still aborts owned readers.

## Bounded software-codec silence

OpenH264 timestamp rate control may accumulate substantial debt after a
recovery picture at the 100 kbps floor. Normal frame skips remain enabled;
if an initialized encoder has emitted no picture for 250 ms of its media
clock, one encode temporarily disables `ENCODER_OPTION_RC_FRAME_SKIP`, then
restores it even after an encode error. This does not force an IDR, rebuild
references, alter geometry, raise a grant ceiling or bypass the serving
writer's existing byte pacing and bounded admission. Under an inadequate
path, transport delivery and quality can still miss their budgets. The
250 ms bound applies to codec skipping when encode calls continue, not
network delivery, UI presentation or physical pixels.

A real Full HD low-rate regression decodes the initial recovery picture and
all subsequent emitted deltas, retains normal skips, and rejects extended
codec silence. Current installed input-to-pixel and mixed-load qualification
remain separate acceptance evidence.

Input-triggered capture retains one pending wake while normal media admission
blocks the producer. Once the admitted producer observes that wake, its 250 ms
burst begins on the current session clock. The pending flag is consumed once;
idle polling resumes after the burst. Accepted input can therefore survive
backpressure that outlasts the original deadline without forcing an IDR,
replaying input, growing a queue or bypassing frame admission. View-only and
failed input do not set that wake. The real-UDP idle-capture regression includes
a 600 ms unavailable-producer interval and retains its existing 300 ms wake
bound after that deliberate pause.

## Decode scheduling diagnostics and cancellation

Slow decode records separate global-budget admission, blocking-pool queue
wait and native codec work, using only the local monotonic clock. Completed
work or a caller that stops waiting after 250 ms emits metadata only: frame
sequence, dimensions, payload byte count, phase and elapsed stage durations.
It contains no image, input, clipboard or peer contents. This distinguishes
resource/scheduling delays from actual native decode cost without extending
existing caller, frame or codec deadlines.

Caller cancellation requests abort of a blocking task that has not started.
If native work is already running, the existing global permit stays inside
that work until it returns; cancellation does not permit an unbounded series
of replacement decoders. Controlled blocking-pool tests cover both cases.
Managed and direct decode use the same work boundary. These diagnostics do
not themselves prove native visible latency or stability under contention.

## Coalesced reference-gap recovery

Repeated successors of the same missing reference may expire while a requested
keyframe is still travelling over reliable QUIC. The receiver now sends one
recovery request for that episode. A received key clears it; an actual rejected
or timed-out reader also permits repair again. Without either outcome, another
request becomes eligible at the existing eight-second key-reader bound. A full
control queue does not arm the hold, and the existing 500 ms request rate limit
still applies. Reader count, memory budgets and read deadlines are unchanged.

Gap records distinguish a sent request from a coalesced observation and include
pending recovery age. Successful frame reads taking at least 250 ms now report
header time, total time, sequence, keyframe flag and byte count. A short scoped
`RUST_LOG=rds_desktop::input_timing=trace` capture can correlate local native
input dispatch with a read-only local image observer, without relying on the
automation tool's clock. It records only sequence, event class and local timing;
key/button codes, coordinates, text and clipboard contents are excluded.
These records locate delay; neither input dispatch nor a frame read proves
physical display response.

## Native work and macOS activity

Slow decode diagnostics now separate elapsed native work from that worker's
thread CPU time. Completed work fixes its end timestamp before reporting, so
later reporting delay cannot inflate the native stage. A failed or unavailable
CPU clock stays absent. Elapsed time minus CPU time includes scheduling and
other native waiting; it is not a diagnosis of a particular OS cause.

The macOS viewer owns a Foundation user-initiated activity for its event-loop
lifetime, including a covered window's active reference processing. It requests
latency-critical timer/I/O precision and prevents automatic idle system sleep;
network I/O cannot continue while the computer is suspended. It does not hold the
display awake or prevent screen locking. The RAII guard ends the activity when
the loop returns or unwinds. Explicit sleep and lid-close behavior remain OS
policy, and reconnection may be necessary afterward. This is scoped application
activity, not a persistent power setting. `pmset -g assertions` can verify the
installed process's assertion. Installed measurements still decide whether it
improves a particular latency episode.

## Obsolete predecessors and repair

A valid header older than the receiver's recovered sequence releases its stream
with `STOP_SENDING` code `0x52445301`. The receiver counts it separately from
malformed or timed-out readers and does not request another key. The sender
recognizes only this exact obsolete disposition, including a stop during an
unfinished write. It frees that frame's admission without ending the video
writer, invalidating the recovered chain or counting fresh delivery for bitrate
growth. Unknown stops and actual failures retain their existing failure path.

No greeting, control message or frame layout changes. Older senders can still
classify the code as generic failure and perform their previous repair behavior;
matching receivers/senders obtain the complete improvement. Limits, malformed
header validation, read/write deadlines and successful-delivery semantics stay
unchanged. Sender and receiver health report obsolete dispositions separately.

Delivery results also retain their frame sequence. A failed receipt older than
an already admitted independent picture cannot invalidate the recovered chain.
The receipt worker owns its repair request; joining the result does not request
repair a second time. A newer produced key suppresses another request for an
older frame, including reference-admission rejection while that key is queued.
Current failures and unknown worker termination still require repair. This
correlation does not treat a produced picture as successful delivery.

Serving-side delivery health and bitrate-reduction records include selected
`path_id`, instantaneous `path_cwnd_bytes` and cumulative sent/received UDP bytes.
Compare byte deltas only within the same path id; these counters include other
services on that connection and do not measure media throughput alone. A soft
delayed-frame warning now has a matching ordinary-level completion record with
sequence, keyframe flag, payload length, `enqueue_ms`, `ack_ms` and `transfer_ms`.
Enqueue spans stream opening/writes and scheduling of the receipt worker; ACK
spans that worker's transport receipt wait. Neither is an application input ACK
or a physical presentation measurement. Timely frame completions remain trace
level. The diagnostic addition changes no pacing, admission or deadlines.

Explicit receiver repair now advances a session-owned media epoch. It retires
obsolete writes/receipt waits without closing control or other connection
services, and forces an independent producer reference. Repeated requests
coalesce until that independent picture is transport-confirmed. Health logs
expose repair generation, accepted/coalesced requests and the active repair gate.

Native keyboard repeat is client-paced: the viewer forwards native OS repeats
only for keys it already forwarded and still holds, without repeating clipboard
side effects. The X11 sink suppresses autonomous per-key repeat during those
holds, pulses an intentional client repeat and restores the original setting
when its last controller releases. This prevents a late KeyUp from manufacturing
letters. Update both native viewer and serving agent together. See the
[regression receipt](reports/rds-interactive-repair-20261004.md) for scope limits.
