# W6 workspace chrome follow-up

The requested increment removes transient toolbar artifacts and provides an
explicit compact mode. Code and native observations determine the diagnosis;
the reported intermittent black pixels have not yet been reproduced locally.

1. Check the actual GPU composition, font texture lifetime and presentation
   mode against pinned egui 0.36.2 / wgpu 30 / winit 0.30 sources. Check idle
   redraw stability and still-picture tab restoration as real native cases. Preserve the
   single latest-frame mailbox and record which defect is actually established.
2. Add a collapse control. Compact mode removes both bars, retains one small
   restore arrow and fits video into the whole window without distortion.
   Keep layout and input coordinates identical, including HiDPI and resize.
3. Keep all arrow/toolbar gestures local, including a release after the layout
   changes. Preserve tab switching, held-input cleanup and clipboard ownership.
   Request follow-up UI paints even when the remote picture is idle.
4. Test geometry, pointer ownership and real native controls with synthetic
   displays; inspect expanded/collapsed/resize states on the GPU. Run the W6
   checkpoint and both platform CI lanes. Keep native evidence distinct from
   physical timing or intermittent-defect reproduction.
5. Update the native contract and report, publish signed atomic changes through
   a PR, then qualify and adopt the exact build using the existing guarded
   private rollout. Synchronize sources and documentation after verification.

Research references (checked 2026-10-10):

- [wgpu presentation modes](https://docs.rs/wgpu/30.0.1/wgpu/enum.PresentMode.html):
  AutoNoVsync prefers Immediate, which permits tearing; Mailbox and Fifo do not.
  AutoVsync can select FifoRelaxed, so it is not a strict no-tearing contract.
- [egui renderer](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/struct.Renderer.html):
  texture updates precede drawing, frees follow it; UI buffers are updated
  before rendering. Existing separate render passes reset their own viewport.
- [egui context](https://docs.rs/egui/0.36.2/egui/struct.Context.html): repaint
  requests belong to the integration, independently of remote frame arrival.
- [winit presentation notification](https://docs.rs/winit/0.30.13/winit/window/struct.Window.html#method.pre_present_notify):
  notify the native window before presenting the submitted frame.
