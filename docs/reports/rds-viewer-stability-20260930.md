# Native viewer stability, quality and text paste increment — 2026-09-30

This scoped W6.1–W6.8/W10 diagnostic increment does not close a remediation
checkpoint, a hardware-codec gate or a full release/mixed-network qualification.
Private endpoint facts, screenshots and raw operational logs stay in the estate.

## Changes

GPU surface acquisition now precedes upload. An unavailable/occluded surface
retains one newest CPU image and cannot accumulate unsubmitted staging writes.
Native startup logs rotate in a private directory, and live snapshots separate
UI dispatch, encoded/decoded/presented age, occlusion and network/render stage.
Input/heartbeat and decode waits have independent bounds. Incomplete encoded
bodies release their reader permits within three seconds and request recovery.

Managed IPC framing has one owned bounded reader independent of a caller's
canceled wait. A regression splitting both message headers and payloads failed
before the fix and passes afterward. Closing the channel aborts that reader.

The per-session video profile uses additive remote `DesktopV3` and local
`DesktopProfile` variants. A macOS native chooser defaults to Full HD; explicit
720p/original choices preserve aspect and do not upscale. Prior wire tags remain
unchanged; older viewers/agents retain legacy behavior or refuse new variants.
Coordinated viewer/agent installation is required for the explicit profile.

Explicit macOS Ctrl+V reads text from NSPasteboard and transfers at most 1 MiB
in ordered 32 KiB chunks before input reaches the remote app. A controlling X11
session owns CLIPBOARD and serves UTF8_STRING/TARGETS/TIMESTAMP plus INCR. Native
workers, requests and reassembly time are bounded. View-only sessions refuse it.
Payloads are redacted even from Debug, never logged or written to diagnostics.
This text path does not implement images/files/rich formats or reverse clipboard.

OpenH264 now receives real monotonic timestamps with timestamp rate control.
Intentional codec skips retain the encoded sequence. The preceding serving-loop
policy produced sequences `(0, 2)` and unnecessary resync; the real-QUIC
regression now observes `(0, 1)` with an ordinary delta. A codec test verifies
reference decoding after a long idle period and monotonic time across rate updates.

The repeated-freeze follow-up removes rate-driven encoder rebuilds. Live native
target/max options retain codec references and avoid large adaptation IDRs;
only geometry changes reinitialize. The direct dependency on OpenH264's already
locked sys crate supplies the exact option types and adds no new codec library.
A sustained RTT increase without packet loss previously reached 100 kbps in the
regression; delay growth now compares successive valid samples. Loss/congestion
and capture deadline signals, the grant ceiling and original floor remain active.
Receive timeouts report frame sequence and partial body byte count, never media
content; rejected incomplete frames also request bounded IDR recovery.

Frame delivery now owns a three-stream transport-acknowledgement budget, below
the receiver's four-reader limit. Outstanding frames reset after five seconds
or writer cancellation. Capture waits for its bounded queue before encoding,
avoiding reference loss and IDR production into a blocked transport. A real
UDP/QUIC regression suppresses peer ACKs after the desktop handshake: the old
implementation continued producing frames, while the bounded sender stops and
resumes on the same connection when ACKs return. This bound trades maximum WAN
frame rate for controlled outstanding work; installed network latency and
recovery remain separate acceptance evidence.

## Verification boundary

Local regression receipts include strict feature compilation, complete workspace
checks, canceled IPC reads, private snapshot hardlink/symlink protection and real
codec/reference tests. Linux native Xvfb checks include Unicode and large INCR
clipboard consumption without a helper process. Installed native input/paste
and actual video dimensions are recorded privately; they are not inferred from
configuration files or source-level checks.

The media test module isolates each declared scenario from unrelated scenarios'
load on process-global frame/decode budgets. Assertions and internal concurrency
(including same-connection desktop+sync) are unchanged. A previous macOS CI soak
observed 199 frames against a 200-frame floor; that failure is retained and is not
relabelled a pass. Isolated tests do not qualify actual mixed-load performance.

Default/all-feature or platform checks not yet completed at any receipt remain
pending. GPU timing is not glass-to-glass latency. Native physical latency,
network/relay/suspend failure matrices, hardware codecs, dynamic geometry,
long-duration quality and exclusive seat ownership keep their declared gates.
