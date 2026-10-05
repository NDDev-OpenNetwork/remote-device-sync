# Controlled keyboard input-to-submission probe — 2026-10-06

W6.3/W10 diagnostic increment: the existing cumulative visual marker can track
explicit unmodified physical KeyDown codes, including a controlled letter and
Backspace. It requires a presented focus click before arming, tracks keyboard
latencies independently, and cancels on focus/target/presentation changes.
Unexpected extra counter increments or modified tracked keys lose correlation.
No remote wire, input injection/replay, transport default or server lifecycle
change. Native config's optional diagnostic path enables an installed app run;
explicit CLI selection wins. No new dependency or unsafe code.

The counter contract requires a verified single controlled actor and one update
per delivered event. Counts and rejection/cancellation flags must be checked.
Only the exact raw image reaching native GPU presentation completes a sample;
network ACK and automation-call completion remain separate boundaries. This
is not optical glass-to-glass qualification or a fix for observed transport
latency tails. Host/runtime facts and two-device runs belong to the estate.

Local macOS CLI desktop library29tests and desktop library115tests passed;
relevant binary targets also compiled/tested. Strict workspace/all-targets
clippy with agent/CLI desktop features passed, plus formatting/diff checks.
The optimized viewer build is scheduled. Cross-platform CI and installed native keyboard measurements
remain required before acceptance. No wider wave is closed by this receipt.
