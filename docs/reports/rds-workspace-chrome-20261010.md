# W6 native workspace chrome follow-up — 2026-10-10

Scope: collapsible local panels, coherent presentation and idle-tab redraw.
The execution and source checks are in [the plan](../workspace-chrome-plan-20261010.md).

## Established defects and changes

The old chrome fed egui-winit's repaint response for RedrawRequested back into
another request_redraw. The native still-picture probe measured 574 redraws
within its initial observation and tens of thousands while covered, despite
only one received frame. The handler now paints that request once and schedules
subsequent frames only for real input, video, surface recovery or egui deadlines.
The corrected probe settled at four initial redraws; later idle observations
retained the same count until interaction.

The old renderer selected AutoNoVsync, which prefers Immediate presentation and
permits tearing. It also accepted a suboptimal surface indefinitely until another
resize and did not schedule a repaint after surface recovery. The viewer ignored
egui's own repaint deadline. This increment selects supported Mailbox or portable
Fifo, keeps the one-frame latency request, notifies winit before presentation,
and handles suboptimal/recovered surfaces and UI deadlines explicitly. No codec,
network, key/grant, system desktop or transport policy changes are involved.

The original report of intermittent black pixels over the toolbar was not
reproduced in the initial native observation. These concrete integration fixes
remove tearing and missed repaint/reconfiguration paths; they do not prove that
every reported black speck had one of those causes. No upstream dependency bug
is asserted and no dependency or shader filtering option is changed speculatively.

A separate black-screen defect was reproduced with the native still-picture
fixture: show each of three display tabs once, then return to the first. Its GPU
texture had been discarded and its only CPU frame already consumed. The screen
remained black until new video, and the native title still named the old tab.
The session now retains one newest shared CPU image alongside its pending
measurement, without a BGRA copy or image history. Idle restoration uploads those
pixels without replaying frame counters, timing samples or visual-probe events.
Switching requests a repaint and the title follows the selected session.

The toolbar up arrow collapses both bars. A 36 by 26 point vector chevron button
at the top center restores them; it consumes no layout height. Video and pointer
mapping use the same full-window content rectangle, preserving aspect ratio.
Pointer ownership includes the current physical overlay rectangle and holds a
local press through release even if the layout or pointer changes. The toolbar
arrow remains reachable at narrow widths. Ctrl+Shift+H provides the same local
toggle outside editing forms. No preference schema change is needed.

## Validation ledger

- Focused renderer/input tests: 49 passed, including shared-image idle restoration,
  replacement/drop ownership, tear-free mode selection, physical overlay hit
  testing and egui layout across 1x, 1.5x and 2x scaling and resize.
- Native macOS still-picture fixture: passed collapse/restore by arrow and
  Ctrl+Shift+H, ordinary/fullscreen resizing and two complete tab cycles across
  two computers and three displays, with one received/submitted frame per tab.
  Upload counts 2/2/3 prove idle restoration, while frame/timing admission counts
  stayed at one. The title followed each tab and restored pixels were visibly
  present. This is native synthetic acceptance, without physical remote input.
- Full registered W6 checkpoint, strict feature Clippy, Linux native qualification
  and publication/installed adoption remain pending in this work-in-progress report.

The fixture is `workspace_preview --still`: two synthetic computers and three
displays, one frame each, synthetic input only. A per-bundle `RDS_PREVIEW_STILL`
environment value enables that same fixture under LaunchServices. It does not
touch an agent, real remote input or connection preferences. The optional
`RDS_PREVIEW_REPORT_DIR` writes one diagnostic log per synthetic tab into a
pre-existing empty report directory and bounds the run to 180 seconds. The ordinary moving
fixture and managed QUIC/H.264 fixture remain available separately.

References checked against pinned versions: [wgpu present modes](https://docs.rs/wgpu/30.0.1/wgpu/enum.PresentMode.html),
[egui renderer](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/struct.Renderer.html),
[egui context](https://docs.rs/egui/0.36.2/egui/struct.Context.html),
and [winit pre-present notification](https://docs.rs/winit/0.30.13/winit/window/struct.Window.html#method.pre_present_notify).
