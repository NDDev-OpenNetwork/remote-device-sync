# Desktop delivery feedback and reference ordering — 2026-10-01

This increment advances W6.4/W6.6 and the W10 diagnostic work. It does not close
hardware encoding, physical glass-to-glass latency, or the complete impairment
and platform gates.

## Failure and change

Live diagnostics showed frame ACK deadlines and a partly received recovery
keyframe while reliable-relay packet loss counters remained clean. The encoder
continued targeting its ceiling. Separately, the receiver expired completed
successors after 100 ms even when an admitted reference reader was still active,
causing avoidable reference loss and new large keyframes.

Ordering now retains at most three successors until admitted readers complete
or reach their existing bounded deadlines. A missing reference with no active
reader still expires after 100 ms. Reader/payload/decode limits are unchanged.

The sender observes each owned frame stream's delivery receipt. A soft delay
threshold of three sampled path RTTs, bounded to 250–1000 ms, reports an impaired
media sample without resetting that frame. Hard ACK/read deadlines are unchanged.
An impaired sample reduces target bitrate by 30%; a five-second hold prevents
immediate reversal. Recovery requires a new successful frame receipt and grows
by at most 1% per 250 ms sample. Existing floor, grant ceiling, three-frame
capture admission and live reference-preserving encoder updates remain in force.
No protocol, identity, permission, or dependency change is introduced.

Normal sender health records delayed-delivery counts and the latest receipt
wait duration. Delay warnings identify sequence and payload length only; no
screen or clipboard content enters diagnostics. Every receipt task and its
reset-on-cancel stream remain owned by the existing bounded writer task group.

## Verification

The real Iroh/Noq regression with a delayed delta reference failed on the prior
implementation: its successor was discarded and delivery timed out. With the
change it delivers both reference and successor in order. The existing delayed
initial-keyframe case now waits 350 ms, beyond the faulty 100 ms window. Both
cases, bounded hostile readers, and independent key/delta deadlines pass.

The gated real-UDP test verifies one held keyframe, bounded capture and
same-connection resumption. Its first rate assertion also passed the prior
controller and did not isolate media feedback. The refined fixture forwards
cadence-resume to its wrapped producer and pauses server egress before datagrams
are emitted, rather than dropping receiver ACKs. With the production pacing
call restored to the old path-only controller, the rate assertion fails; with
media feedback enabled it passes. Both outcomes are retained. Controller tests
also cover clean relay counters plus delayed media, the recovery hold, no growth
without fresh ACKs, bounded growth, and floor/ceiling preservation. The
process-wide hostile-reader fixtures are serialized so independent scenarios do
not consume each other's global budget; concurrency and budget assertions within
each scenario are retained.

Mac whole all-feature workspace tests pass: **775 passed, 0 failed, 2 ignored**
across 107 result groups. Strict all-feature/all-target workspace clippy,
formatting and cargo-deny advisories/bans/licenses/sources pass. The two ignored
cases remain outside this increment's acceptance. The fixture refinement changes
only tests; the installed production code is unchanged.

Linux whole all-feature workspace tests pass: **777 passed, 0 failed,
11 ignored**, across 107 result groups, together with strict all-feature
workspace/all-target clippy and formatting. Ignored native/display/account cases
are not counted as passes. These checks ran on the production commit; the
subsequent test-fixture refinement has its separate targeted receipt.

Installed-device observations belong to the private estate. The prior longer
run contained an unplanned recovery and remains failed stability evidence. A
new installed run is being qualified; a locked local console is excluded from
visible-pixel acceptance. No full milestone gate is marked closed here.
