# Bounded low-rate codec starvation — 2026-10-01

An installed Full HD session recorded a 9,760 ms decoded-picture pause.
Server health showed rate-control skips increasing from 71 to 182 while
picture production stayed unchanged for several seconds at 100 kbps and
selected-path RTT had returned to 114–123 ms. A prior DHCP/network-unreachable
pause remains a separate observation; this later episode directly identifies
software encoder debt after a recovery picture. This does not establish that
all network or native presentation problems share that cause.

A deterministic real OpenH264 1080p regression emits a complex recovery
picture at 100 kbps, then continues small physical image changes at 17 ms
media timestamps. The previous implementation fails the 250 ms no-output
bound after 255 ms. The new encoder uses the documented live frame-skip
option for one encode once that bound is reached, then restores ordinary
rate control even when encoding fails. It does not request an IDR or discard
references. The fixture decodes each emitted delta and verifies subsequent
normal skips. Serving transport pacing/admission, grant bounds and decoded
newest-frame presentation are unchanged.

Current software-codec timing is distinct from input-to-pixel acceptance.
The prior 30-minute installed observation received 14,912 Full HD frames with
zero reconnects, but freshness p95/max 1,725/9,760 ms failed. Most native
presentation samples were OS-covered, so that run does not qualify visible
response. The updated codec's installed two-device qualification and full
native latency/quality/stability gates remain open.

Validation so far: all 63 macOS desktop units pass, including continuous
references, live rate updates and the new actual-codec case. Strict full
workspace/all-targets/all-features clippy and formatting pass. Integration,
Linux and installed-binary results are recorded as they complete; they are
not implied by this focused result. No milestone checkpoint is closed.
