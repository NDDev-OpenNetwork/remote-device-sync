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
retains at most three successor bodies; a gap lasting 100 ms requests bounded
IDR recovery. The existing global encoded/decode limits still apply.

The viewer reopens an interrupted desktop channel with a fresh session route.
When the managed connection disappeared, it reconnects the same pinned peer
through the manager. Reconnect backoff is bounded to eight seconds; inputs
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
Incomplete media bodies release reader permits after three seconds and request
IDR recovery; handshake/control deadlines remain separate. Receiver health logs
include admission, completion, rejection, timeout, gap and keyframe counters.
