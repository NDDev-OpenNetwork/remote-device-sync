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
or reach their existing bounded deadlines. A missing reference with no active reader
uses two measured control RTTs, bounded to 100–1000 ms, with a 250 ms initial
fallback. Reader/payload/decode limits are unchanged.

The sender observes each owned frame stream's delivery receipt. A soft delay
threshold of three sampled path RTTs, bounded to 250–1000 ms, reports an impaired
media sample without resetting that frame. Hard ACK/read deadlines are unchanged.
An impaired sample reduces target bitrate by 30%; a five-second hold prevents
immediate reversal. Correlated media receipts coalesce over one second; a
simultaneous path penalty is combined by taking the stronger response once.
Continued pressure after that second can reduce load again. Recovery requires
a new successful frame receipt and grows
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

At the initial production commit, Mac whole all-feature workspace tests passed:
**775 passed, 0 failed, 2 ignored**
across 107 result groups. Strict all-feature/all-target workspace clippy,
formatting and cargo-deny advisories/bans/licenses/sources pass. The two ignored
cases remain outside this increment's acceptance. The fixture refinement changed
only tests. Subsequent media-burst adaptation is qualified separately below.

At that initial production commit, Linux whole all-feature workspace tests passed:
**777 passed, 0 failed,
11 ignored**, across 107 result groups, together with strict all-feature
workspace/all-target clippy and formatting. Ignored native/display/account cases
are not counted as passes. These checks ran on the production commit; the
subsequent test-fixture refinement has its separate targeted receipt.

The longer run exposed excessive rate reductions from correlated receipts and
overlapping path/media signals. A regression reproduced two 30% cuts for one
sample (4 Mbit/s became 1.96 instead of 2.8 Mbit/s). The controller now combines
the responses once and coalesces media cuts for one second. The regression also
verifies that sustained pressure after that second still lowers offered load.
The full desktop test suite passes the new code; updated whole-workspace and
installed-device observations will be recorded after qualification.

Installed-device observations belong to the private estate. The prior longer
run contained an unplanned recovery and remains failed stability evidence. A
new installed run is being qualified; a locked local console is excluded from
visible-pixel acceptance. No full milestone gate is marked closed here.

A real Iroh/Noq test also delays the missing reference tag beyond 100 ms while
measuring a 200 ms control RTT. The former fixed timer requested an unnecessary
IDR before that reference arrived. The receive loop now uses a bounded RTT-based
window and logs the expected sequence and expiration budget on actual gaps.
No payload or identity is added to these diagnostics. The initial test harness
mistakenly requested a legacy route while expecting V2; that failed setup was
retained separately and excluded from the corrected failed-before evidence.

A separate five-second production health record remains active when the writer
has no new frame. It reports native production activity, intentional codec skip
count, latest produced-frame age, admission/keyframe waits and path RTT. These
metadata distinguish capture/encode inactivity, intentional skips and transport
backpressure during the observed long pauses. Hard budgets and wire remain
unchanged; no image, clipboard content, credential or endpoint identity is logged.

At the WAN/progress-diagnostics revision, Mac whole all-feature workspace tests
pass **778/0/2 ignored**,107 result groups; Linux whole recheck passes
**780/0/11 ignored**,107 groups. Strict all-feature/all-target workspace clippy
and formatting pass both. One first-run Linux owned-relay closure assertion
failed under host load; the unchanged focused case and full recheck pass, and
both raw outcomes remain recorded. Current both-OS CI also passes. Ignored
platform/account cases remain outside these totals and acceptance claims.

The independent logs exposed a clock-origin bug in managed heartbeats: verbatim
viewer timestamps survived reconnect, while the desktop receiver subtracted
its own new session clock. RTT saturated to zero and therefore used the100 ms
reorder floor. Heartbeat timing now correlates up to64 sent sequence/timestamp
pairs with local monotonic instants. Echoes preserve the caller's wire values;
unmatched or duplicate echoes cannot invent an RTT. A real managed-style
heartbeat test with a different timestamp origin failed before and passes with
the fix. Bounded-correlation tests cover eviction, exact matching and duplicates.

After the heartbeat clock-origin repair, both whole all-feature suites pass:
Mac **779 passed, 0 failed, 2 ignored**; Linux **781 passed, 0 failed, 11 ignored**,
107 result groups each. Strict all-feature/all-target clippy and formatting pass.
The scoped real-device qualification remains separate: prior failed motion runs
are retained and the corrected build has a fresh visible native/mixed-workload
run underway. No broad milestone or physical-latency closure is inferred from
these checks.
