# Heartbeat control-stage observations — 2026-10-06

W10 diagnostics: exact heartbeat sequences now expose client write, server read/
reply and client matched-read metadata. EOF/errors/incomplete writes retain an
explicit termination boundary. This supplies stage evidence for a live-picture
or whole-desktop control gap; it does not identify a transport root cause by
itself. Pending read includes idle/scheduling and is not a network latency.

No protocol, input semantics, endpoint settings, timeout, retry or resource
ownership change. Numeric metadata only. The existing bounded telemetry output
owns writing outside interactive tasks. Owning attempt/connection context and
clock qualification remain necessary for joins.

Local macOS desktop104library tests and strict workspace/all-targets desktop
clippy passed, plus formatting/diff checks. Cross-platform CI and installed
trace qualification remain required. No native
latency or full stability gate closes from additional traces.
