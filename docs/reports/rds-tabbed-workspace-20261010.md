# Native workspace implementation — 2026-10-10

Status: implementation checkpoint and bounded native acceptance pass, W6/W9.2.
Final artifact and installed/platform/network qualification remain distinct.

Before this wave, the viewer could launch up to eight independent processes for
explicit device/display pairs, without a tab bar, picker or in-window settings.
This wave implements one convenient workspace with multiple devices and
multiple displays, independently configurable and switchable without disconnecting
other sessions. Existing managed session identities, monitor inventory, bounded
input/clipboard queues and per-session desktop routes remain the transport owners.

1. Add a bounded workspace model with stable tab IDs, exact device/display
   identity, selection/close/reorder rules and per-tab quality/input/clipboard
   preferences. Reject duplicate conflicting intent and stale async replies.
2. Reuse the existing presentation, input translation, clipboard and reconnect
   worker for every tab. One native window/event loop/GPU owns visible rendering.
   Switching releases the old tab's held keys/buttons before routing new input;
   inactive tabs cannot publish clipboard or show another tab's texture.
3. Add a tab bar and connection/settings UI. Inspect a device through the local
   agent, list actual displays, open one or several, and isolate failures to the
   affected tab. Never create endpoint identities, issue grants or widen policy.
   Existing separate-window/headless/diagnostic modes remain explicit options.
4. Persist ordinary workspace/profile preferences with a versioned bounded
   atomic file, containing no embedded credential. Import existing viewer launch
   configuration without rewriting it. Show save errors and unsupported displays.
5. Test state ownership, stale completion, switching with held input, clipboard
   focus, viewport coordinates, reconnect/settings isolation and multi-device
   session coexistence. Verify native UI on macOS and isolated Xvfb on Linux,
   then workspace/feature checks, a registered checkpoint and an exact artifact.
6. Reconcile signed PR source, estate pin and installed runtime only after the
   relevant qualification, retaining the separate known control RTT failure.

Implementation references: the maintained [egui integration](https://github.com/emilk/egui)
and [egui-wgpu 0.36.2](https://docs.rs/crate/egui-wgpu/0.36.2) use the existing
wgpu 30 renderer family. A small optional UI dependency is preferable to owning
custom text shaping, widgets and focus management. Exact compatibility, licenses
and feature footprint must pass before adoption. References are checked against
the actual October 10 implementation, not asserted as archived September 26 pages.

The local sync checkpoint at `89db733` passes 1,072 executions; PR150's macOS
desktop-feature RTT p95 411 ms remains above the unchanged 400 ms bound. Native
build success is not acceptance of that latency requirement or of this new UI.

## Current checks

Four model tests passed for independent devices/displays, duplicate intent,
close/reorder, stale revisions and settings validation. Two preference tests
passed for atomic replacement, existing readers, private mode, retained legacy
configuration, aliases, oversized files and invalid references. The desktop/CLI
feature library suite subsequently passed 173 tests (32 CLI + 141 desktop),
including shared authenticated peer binding and the common viewport bounds.
Strict feature Clippy passed before the latest small validation/doc additions;
final checks still need to bind the complete reviewed source.

The isolated macOS native preview opens three synthetic displays across two
computer profiles and never contacts an agent or network endpoint. Actual UI
interaction verified mouse selection, Ctrl+Tab, changing one display to 60 FPS
while its sibling remained at 30 FPS, closing that display while the second
computer remained visible, and reopening it from the monitor picker. The preview
was closed; the installed RDS application was not replaced or terminated. This
does not qualify actual remote capture, input injection or clipboard transfer.

The new dependency check initially refused the embedded font crate's OFL-1.1
and Ubuntu-font-1.0 requirements. The fix retains the exact four upstream notices
with file hashes, makes them viewable in About and includes them in native app
packaging. The license exception is scoped to `epaint_default_fonts =0.36.2`;
the repository-wide license allowlist remains unchanged. Reviewed primary
[OFL conditions](https://openfontlicense.org/open-font-license-official-text/)
and the bundled Ubuntu font license, with the official
[cargo-deny scoped-exception contract](https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html#exceptions).
The fonts remain unmodified. Repeat the full all-feature dependency check.

## Source-bound checkpoint and managed native sessions

At clean `3fe2a49`, the first registered workspace checkpoint passed 1,129 test
executions, zero failed and two explicitly ignored. The original command for
`multiple_desktops` selected Iroh only: Noq is gated by the owning client's
feature, not merely by a transitive transport feature. The gate and CI now
explicitly enable `rds-client/transport-noq`; the follow-up passed both internal
backend cases. The earlier generated report was corrected rather than claiming
coverage it did not run. The all-feature dependency check also passed after
the exact font-notice change.

The release model benchmark used 100 samples of 1,024 selection/configuration/
reorder/close/reopen transitions each at 1, 4 and 8 open tabs. Batch p95 was
66.833, 84.250 and 95.333 µs. Every sample checked capacity, stale revisions,
non-reused identity and empty final ownership. These are state-model timings,
not GPU, network, input-to-pixel or memory-soak results.

The `workspace_managed_preview` fixture at `7481fb4` then exercised the production
workspace orchestrator through a real same-UID local manager, two loopback peers,
three independently encoded H.264 displays and synthetic input sinks. It never
captures an OS display or injects input into another application. Two fixture
setup failures are retained: an overlong macOS temporary socket path and a
dispatcher that initially handled V5 but omitted the correctly requested V4.
Both were corrected before reporting successful qualification.

| Backend | Display | Opens / closes | Requested FPS | Produced frames | Scoped input events |
|---|---|---|---|---|---|
| Noq | First peer, 0 | 1 / 1 | 10 | 2326 | 11 |
| Noq | First peer, 1 | 2 / 2 | 10 → 20 | 2485 | 7 |
| Noq | Second peer, 0 | 1 / 1 | 10 | 2327 | 7 |
| Iroh | First peer, 0 | 1 / 1 | 10 | 2325 | 9 |
| Iroh | First peer, 1 | 1 / 1 | 10 | 1148 | 7 |
| Iroh | Second peer, 0 | 1 / 1 | 10 | 2326 | 31 |

Both 240-second runs retained the manager's original endpoint identity and ended
with matching open/close counts and no held synthetic keys. Native interaction
changed only the selected Noq display to 20 FPS; its siblings were not reopened.
On Iroh, closing one display left both remaining views/input paths functioning.
Saving persisted exactly those two remaining tabs with mode 0600. A live Iroh
observation recorded 1,649 received and 1,567 submitted frames on the visible
second peer, zero pending input acknowledgements, and an actually visible native
window. The hidden first peer continued receiving with no visible-submission
claim. During the Noq run, macOS initially reported the window occluded despite
fresh decoded frames; only after entering full screen were fresh presentation
and matching tab colors verified. Cached screenshots were not treated as live
presentation evidence.

This qualifies the loopback native/manager/transport/codec path on macOS. Linux
native execution, installed fleet acceptance, physical capture/input, cross-tab
clipboard handoff and sustained resource/latency qualification remain separate.
The later empty-workspace Ready/activity fixes also require the final artifact;
an earlier compiled candidate is not relabeled as that source.

## Combined source checkpoint

The registered checkpoint on clean `f686ac2` passed 1,130 test executions,
zero failed and two explicitly ignored native cases. It includes explicit
Noq client-feature selection for the two-backend coexistence case. The
18-record receipt chain verifies. The 100-sample 1/4/8-tab model benchmark
measured batch p95 48.833/96.292/106.834 µs on this run; it remains a model
measurement, not native GPU/network latency. The macOS production binaries
also built successfully from exactly that source with Rust 1.98.1.

Source review confirmed that an authorized Copy reply could remain queued in
a hidden tab because only the active mailbox was drained. That correction
is a separate source change, and this earlier checkpoint/build does not
qualify it. Neither candidate has been described as installed acceptance.

## Final implementation receipt

Clean `94260b1` passes the full registered checkpoint: 1,136 passing executions,
zero failed and two explicit native ignores, with an intact 22-record receipt
chain. The final native clipboard/input/lifecycle corrections and actual A/B
results are recorded in [the handoff report](rds-workspace-clipboard-20261010.md).

On `5c3cf0c`, separate 15-second Linux Xvfb managed previews for Iroh and Noq
opened, produced encoded pictures and closed all three display sessions while
retaining the local manager identity. No synthetic keys remained held. Those
headless functional probes are not hardware/FPS or physical-input acceptance,
and do not replace the final source's complete platform qualification.

## Connected-device discovery follow-up

Installed native acceptance exposed a duplicate computer card when the saved
profile used a connection ticket and the local manager reported the same peer
as a bare endpoint ID. Startup discovery compared the two serialized strings.
The new regression fails against that comparison and passes when discovery
compares the endpoint identity through the existing target parser.

The saved profile key, ticket routes and external grant reference are retained.
This only suppresses an automatically added duplicate; it does not merge
explicit authorization profiles or resolve unknown names by guessing. Different
peers and malformed/unresolved targets remain distinct. The client library's
33 desktop/Noq feature tests pass, including alternate routes for the same peer.
The workspace model and its recorded transition benchmark are unchanged.

The identity/routing distinction matches the upstream
[EndpointAddr model](https://www.iroh.computer/blog/iroh-0-94-0-the-endpoint-takeover),
and was checked against this repository's actual `EndpointAddr`, `Ticket` and
`parse_target` implementations. The installed observation establishes visible
connection, settings and real display inventory; multiple physical computers,
clipboard applications and sustained network acceptance retain their separate
qualification boundaries.
