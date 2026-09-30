# Native desktop viewer

The `desktop` CLI feature builds a native winit window with a wgpu surface:
Metal on macOS and supported native GPU backends on Linux. Software H.264
decoding stays on bounded blocking workers. The serving device needs a real
capture/input backend; the current Linux implementation uses X11/XTEST.
macOS capture, VideoToolbox and Wayland serving remain separate work.

`--always-on-top` keeps the remote screen visible above other application
windows. This is useful while using a local app alongside the remote desktop
or observing pixels during qualification. Normal windows retain the OS stacking
behavior, and an occluded Metal surface may pause presentation. Returning focus
or uncovering the window requests an immediate redraw of its latest image.

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
keyboard layout interprets them. IME/text composition, clipboard/audio and
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

The package contains the native executable and generic Info.plist. Its local
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
No screenshot, peer key, address or credentials are
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
