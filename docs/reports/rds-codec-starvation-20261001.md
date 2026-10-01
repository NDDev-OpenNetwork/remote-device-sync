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
workspace/all-targets/all-features clippy and formatting pass. Relevant macOS desktop/client/CLI integrations pass 136 cases (one platform
ignore) and Linux passes 140 (ten ignores), across 24 groups per platform.
Linux strict full workspace lint and the serving release build pass. Installed
binary and native pixel qualification are recorded separately as they complete. No milestone checkpoint is closed.

The serving binary is installed on the existing Linux endpoint with the
original credential/policy and desktop seat. The already-running native
Mac client recovered in 1,235 ms after the planned serving restart and
continues decoding Full HD. Its window is outside the active unlocked
console; this recovery is network/decode evidence, not visible-pixel
qualification. A subsequent bounded installed continuity run is in progress.

A subsequent 600-second installed decoder observation received 2,624 Full HD
frames with zero reconnects and about 69 MiB peak resident memory. Freshness
p95/max remained 920/5,214 ms, so this continuity check failed. All native
presentation observations were OS-covered. No physical-pixel result is
claimed, and this shorter different workload is not a comparable before/after
performance benchmark.

A separate managed-channel fixture, concurrent with the existing native
session, verified 12 attributed target transitions on each run. It traverses
real local IPC, RDS control/QUIC, native input/capture/encode and the Mac software
decoder; it excludes native key handling, Metal and physical display. Normal
input-to-decoded-color p50/p95/max was 695/1,040/1,684 ms. A forced 100 kbps
run measured 669/1,732/2,551 ms; all transition pictures were deltas of roughly
2 KiB. These are functional transitions with unacceptable latency tails.
Normal local input-ACK median was 348 ms; same-server-clock handled-to-capture
and capture-to-encode medians were 32 and 66 ms. Some low-rate captures still
started over a second after handling input while media admission was blocked.
Codec-skip progress alone therefore does not close interaction or quality gates.

The macOS full CI lane exposed a separate owned-transport telemetry fixture
timeout after path churn. Only its intentionally delayed observer should lag;
the ordinary facade must observe each established path before the fixture
closes it and advances. An explicit observer barrier retains both the sticky
loss and single-live-path assertions and the existing timeouts. All 85 local
network units pass with that barrier; the failed CI artifact is retained and
fresh full CI remains separate evidence.

## Accepted input pending across capture backpressure

Same-clock target-transition instrumentation also found accepted-input to
capture gaps over one second. The earlier input hint expired 250 ms after
handling, including time when media admission prevented capture. A bounded
pending flag now survives that interval and rebases the short capture burst
when the admitted producer first observes it. No command is replayed and no
additional frame may bypass admission.

The real-UDP idle-capture fixture adds a 600 ms producer pause. With only the
old absolute-deadline check, accepted input never produces the requested delta
within the original 300 ms wake allowance after that pause. This failed before
the fix; the pending-wake implementation retains the view-only denial and
continuous-delta/no-IDR checks. Its final software/installed results are being
recorded separately; this change does not claim native visible latency success.

The pending-wake runtime `1131e0e` passes the relevant desktop/client/CLI
lanes: 137 macOS cases (one platform ignore) and 141 Linux cases (ten ignores),
across 24 groups each. Strict whole-workspace lint and the serving release
build pass. Full two-OS CI and native builds pass for that runtime revision.
The existing serving agent is updated; the same native client resumed decode
in 1,793 ms after the planned restart.

Installed follow-up preserves negative observations: one low-rate run hit the
65-second overall observer limit, another failed its initial-target deadline,
and a normal run completed all 12 transitions but had p50/p95/max of
1,038/1,391/1,596 ms. Improved probe logs retain partial samples and explicit
connect/open/initial-target/input/finish stages on failure. A subsequent
low-rate run completed 12 attributed transitions and clean channel shutdown,
with p50/p95/max of 496/698/2,404 ms. Network/workload conditions differ between
runs, so these are separate observations rather than a controlled performance
improvement claim. Current native visible latency, quality and sustained
interaction remain unqualified; the Mac console was still locked.
