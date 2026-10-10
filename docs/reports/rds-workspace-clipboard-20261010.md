# Workspace clipboard handoff — 2026-10-10

Status: reproduced by source review; correction and qualification in progress.
Milestone: W6/W9.2 native workspace input ownership.

The workspace retains each hidden tab's clipboard mailbox, but only drains the
active mailbox. A Copy followed by selecting another tab can therefore strand
an already authorized reply. The clipboard state already permits a bounded
explicit-copy handoff after window focus loss; the workspace must preserve that
contract without allowing an unsolicited hidden session to publish.

Plan and acceptance:

1. Drain authorized hidden clipboard work on the native event-loop thread.
   Keep one bounded shared clipboard owner while that handoff is pending.
2. Treat a newer explicit Copy/Cut in any tab as superseding older tab work.
   Cover Control shortcuts as well as macOS Command translation. Native
   generation changes also invalidate stale publication.
3. Keep paste bound to its selected session and original gesture. A pending
   handoff must never paste stale local text or replay into a different tab.
   Cancel deferred work on new interaction, tab/session change, focus loss or
   a bounded deadline; report failure rather than falling back to stale text.
4. Exercise delayed offers/chunks, competing copies, native generation changes,
   expiry and original modifier intent with deterministic regressions, then
   feature Clippy/library checks and the registered workspace checkpoint.
5. Update the native contract and source-bound report. Native OS clipboard and
   multi-device acceptance remain distinct from a deterministic state test.

Primary references reviewed on October 10 (not represented as archived
September 26 pages): [Apple pasteboard changeCount](https://developer.apple.com/documentation/appkit/nspasteboard/changecount)
identifies changed ownership; [ICCCM selection transfer](https://xorg.freedesktop.org/archive/X11R7.7/doc/xorg-docs/icccm/icccm.html)
defines owner/requestor cooperation; [winit 0.30.13 events](https://docs.rs/winit/0.30.13/winit/event/enum.WindowEvent.html)
separate focus, keyboard and occlusion. These contracts support explicit
ownership and cancellation; they do not prove a particular latency bound.
