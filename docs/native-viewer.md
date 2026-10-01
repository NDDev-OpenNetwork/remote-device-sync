# Native desktop viewer

The `desktop` CLI feature builds a native winit window with a wgpu surface:
Metal on macOS and supported native GPU backends on Linux. Software H.264
decoding stays on bounded blocking workers. The serving device needs a real
capture/input backend; the current Linux implementation uses X11/XTEST.
macOS capture, VideoToolbox and Wayland serving remain separate work.

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
a release. Consecutive pointer moves collapse without crossing key/click order.

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
tracked until it completes or its owned task is canceled. A completed delayed
receipt remains a diagnostic count and is no longer treated as a current queue.
Soft congestion requires two consecutive 250 ms samples with outstanding late
receipts and no new successful receipt. A hard delivery failure still reduces
load immediately. A five-second recovery hold and growth of at most 1% per
sample with fresh receipts prevent immediate return to a sustained backlog.
Media reductions coalesce over one second; simultaneous path/media observations
apply the stronger response once. Existing bitrate bounds, frame deadlines,
three-frame admission and reference-preserving live encoder updates still apply.
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
decoded frame triggers a new desktop session. Closing the window cancels and
joins its network worker, leaving unrelated managed streams intact.

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
GPU upload count, reconnects and stage latency. Snapshots contain no image,
clipboard, peer or credentials. Stalled input/heartbeat writes and decode waits
are bounded separately; reconnect causes and panics are recorded. `--report`
still emits an end-of-run receipt, while the live snapshot survives a hung or
terminated UI. CLI desktop commands may opt in with `--diagnostics-dir` pointing
to an existing private directory. Independent UI probes share the bounded wake
flag; a stalled UI cannot create an unbounded event queue.

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
using the 2% loss threshold. A short burst losing at least five packets and
more than 10% can react earlier. Individual transport congestion events remain
diagnostic: QUIC still handles retransmission and packet pacing. A large RTT
rise needs two consecutive 250 ms observations; path and media penalties for
one burst share a one-second cooldown. Actual failed media receipts and
sustained blocked delivery retain their bounded recovery and bitrate response.
This prevents a single lost packet among a handful of idle-screen datagrams
from repeatedly collapsing Full HD quality.

Private normal-level server logs now retain every bitrate reduction's path
counters, delivery state and producer misses. Five-second health records also
include direct/relay selection and the maximum successful input-injection
duration in that interval. No input values or content are recorded. Viewer
reports correlate up to 128 sent input sequences using only the viewer's local
clock, with bounded 1024-sample histories. Input ACK p50/p95 includes local
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
