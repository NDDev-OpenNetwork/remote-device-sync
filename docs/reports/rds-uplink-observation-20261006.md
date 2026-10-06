# Desktop client transmit observations — 2026-10-06

W10 diagnostics increment: weak backend-neutral path observations accompany
slow matched control replies. Each desktop control lifetime has a numeric local
instance ID. Bounded observation work retains no connection facade and never
awaits queries on input/receipt writers or the event reader. An already-running
synchronous query can finish after cancellation; its age and work duration are
explicit. Worker failure affects diagnostics rather than session lifecycle.

Transmit loss and deltas are produced at the viewing endpoint. A server transmit
counter cannot establish viewer transmit loss. Missing/reset baselines remain
unknown; at most eight live paths are retained and selected paths are preferred
when reporting explicit truncation. No transport setting, protocol, timeout,
identity or authority change; no addresses, input/clipboard bodies or credentials.

The existing real connection-lifecycle fixture now also retains this public weak
observer across last-facade drop, checking it cannot keep transport I/O alive on
Iroh and owned Noq. Diagnostic arithmetic tests cover unknown/reset counters.
macOS strict workspace/all-targets desktop/Noq Clippy passed. The final targeted
desktop/transport suite passed 307 tests with zero failures. The first attempt
failed at linker ENOSPC before completing tests; the guarded retry completed
with exit0 and no free-space guard trigger. Cross-platform qualification
and installed trace evidence remain required. This is a diagnostic gap repair,
not a claim that residual native input pauses are eliminated.
