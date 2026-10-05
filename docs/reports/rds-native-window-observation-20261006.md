# Native window observation — 2026-10-06

This W6.3/W10 diagnostic increment distinguishes the native viewer's own macOS
window state from stale winit occlusion evidence. It does not close the
input-to-visible latency or installed stability gates.

The snapshot adds optional AppKit flags and monotonic sample age. Reading occurs
on the OS main thread through the existing thin objc2 bindings, outside the state
mutex. No window content, title, geometry or other application state is recorded.
Other platforms report no native sample. Wire formats, endpoint settings and
renderer occlusion/recovery policy are unchanged. Identical status/title writes
are skipped on frame wakeups.

Relevant native contracts are Apple's [active Space property](https://developer.apple.com/documentation/appkit/nswindow/isonactivespace)
and [occlusion state](https://developer.apple.com/documentation/appkit/nswindow/occlusionstate).
Neither historical occlusion nor network ACK establishes physical visible
response; a fresh sample and the frame/submission timeline must be correlated.

Local macOS arm64 validation:

- CLI desktop library: 29 tests passed.
- Desktop library under the same feature graph: 112 tests passed.
- Workspace/all-targets strict clippy with desktop features passed.
- Formatting and diff whitespace checks passed.
- Optimized CLI/viewer build passed (2m51s).

Linux/macOS CI and installed typing/Backspace qualification are pending at this
receipt. No benchmark result or freeze root cause is claimed from compilation.
No remote desktop session or application restart is required by this increment.
