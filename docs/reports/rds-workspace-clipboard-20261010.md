# Workspace clipboard handoff — 2026-10-10

Status: implementation, 43 render regressions and strict desktop/CLI Clippy pass;
native handoff and complete combined checkpoint remain in progress.
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

## Implemented boundary

A single workspace clipboard owner drains the selected or authorized hidden
mailbox. Explicit Control/Command Copy/Cut revokes other tabs' pending transfers.
The AppKit generation check prevents an old result from replacing a newer local
copy. The ordinary single-window path shares the same session work function.

One deferred paste waits at most five seconds and is bound to destination tab,
settings revision and live connection epoch. New input, focus loss, changed
native ownership, closing/restarting a relevant tab and expiry cancel the
gesture. Its original Shift intent is restored for the synthetic Control+V
chord while every currently held modifier is retained afterwards. Physical V
release/repeat cannot replay the consumed paste. Disabled workspace clipboard
no longer reads or uploads native text; the remote paste shortcut still works.

Deterministic checks cover the hidden authorized handoff, superseded replies,
local ownership changes, expiry, preservation of fresh focused offers,
destination/session staleness and delayed modifier transitions. Initial strict
Clippy found a collapsible conditional; corrected code passes without a lint
exception. These checks are not native cross-tab clipboard or installed
qualification. Linux native clipboard publication remains explicitly unavailable.

## Native integration review

The native three-tab preview on production source `6008b42` accepted two delayed
Copy→switch→Paste gestures with exact synthetic text, including a newer Copy
superseding the first transfer. A later pending paste canceled on Escape.
The preview's macOS Quit action exited before its final report writer, so these
observations are retained as live counter/UI evidence, not final shutdown proof.

The same run reproduced an input-routing defect: ordinary Tab moved focus to
the local tab button and produced no remote input transition. Pinned
[egui-winit 0.36.2](https://docs.rs/egui-winit/0.36.2/src/egui_winit/lib.rs.html)
processes keyboard side effects before reporting consumption, always consumes
Tab and reads native Paste even when no local text widget owns the keyboard.
The integration now bypasses local key-press handling while the remote view
owns input. Releases still clear old widget state after a local dialog closes;
they cannot consume the remote release. Winit focus-generated synthetic keys
are ignored before either local or remote gesture routing. Native revalidation
and the new source's complete checks remain required.

The keyboard-routing candidate's native run delivered ordinary Tab down/up to
the synthetic remote sink (the baseline delivered neither), and again recorded
two exact-text pastes without unexpected content. Its timed shutdown then
reproduced a debug panic from one unpainted `TexturesDelta`; no final report was
written, so the run is not classified as successful shutdown acceptance.
A dedicated regression fails before the fix. `UiFrame::drop` now explicitly
clears CPU-side texture commands when their window/GPU owner ends. Live texture
uploads/frees still pass through the painter. All 44 render tests pass after
this correction; the native run must be repeated through its normal deadline.
