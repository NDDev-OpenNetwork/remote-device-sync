# Prepared Wayland backend boundaries — 2026-10-11

Scope: optional Linux portal serving and its generic agent ownership seam.
This report does not establish live compositor, physical input or deployment
acceptance. The [contract](../wayland-portal.md) owns prerequisites and limits.

Registered w4-wayland-portal passed on the Linux candidate: 1007 test executions,
zero failures and three explicit native-only ignores, default/x11/portal strict
Clippy, nine original portal boundary cases and the agent library. Its
[100 shared endpoint handshakes](bench-w4-wayland-portal.md) are a regression
measurement, not Wayland capture or input latency. The receipt chain has 30
entries and retains the actual public base, dirty state, lock and binary digest.

Subsequent targeted validation covers the final CLI preflight and worker exit
changes: 10 configuration cases on Linux/portal and macOS/desktop; 12 portal
cases including real EI sockets, held-key sharing and pause invalidation,
private state rotation/confinement native worker unwind guards and a private D-Bus pending-request Close. Strict
portal agent/desktop Clippy passed. macOS default and desktop-serving workspace
Clippy passed; hosted CI still applies to the published final commit.

Retained draft failures include a missing serde_json feature dependency,
feature-absent constructor/preflight compilation, Clippy issues, and a fixture
which incorrectly sent EI region_mapping_id after region. The corrected fixture
follows the protocol ordering and validates two offset/fractional-scale regions.
No timing threshold, input workload or parser check was weakened. A local X11
fixture attempt also lacked Xvfb. After installing only that test package, the
isolated two-screen fixture passed 14 native capture/input/clipboard cases.
The shared catalog rejects the XWAYLAND extension even with missing session
environment metadata; no ambient desktop receives fixture input.

Attended native capture/decode, permission restore/revoke and scoped physical
input remain OPEN. Consent preparation is explicit and local; a remote client
cannot open a dialog. Clipboard and relative pointer input remain unavailable
for this backend. Hardware encoding, unconsented login-screen access, hotplug
identity acceptance and physical timing are not claimed.

Prepared-source inventory now respects the agent service policy: four real
QUIC service-policy cases passed, including a counted source which is never
probed while Desktop is disabled. Node 0/ANY and ambiguous identities are refused.
Input checks lease/era/deadline before flushing and before accepting a late
native sync response; bytes already written cannot be rolled back.

The convenience SDK Start awaited user Response before returning its Request,
which made explicit dialog cancellation unavailable during the wait. The native
boundary now subscribes first and uses the standard Start with a retained own
request path. Setup errors join cleanup; Request.Close precedes Session.Close
and unique bus shutdown. A private D-Bus fixture verifies actual Close dispatch
without a user response. This repairs cancellation ownership; it does not prove
why a particular installed request failed to return its Response. Token-bearing
responses are never logged. Local consent is user-paced with a one-hour bound.

The first published CI workflow failed validation before creating jobs:
[run 38103876464](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38103876464)
identified an unquoted `portal::` test filter in YAML. A folded command scalar
preserves the exact test invocation. YAML parsing and actionlint passed after
the correction; the failed run remains evidence, not completed Rust validation.

The subsequent Noq Clippy lane found a priority fixture still passing the old
boolean desktop argument. The fixture now uses a disabled `DesktopBackend`,
preserving its original policy and workload. Both control/media priority cases
and strict workspace Clippy with the combined owned-transport/desktop features
passed locally. The original hosted compilation failure remains in
[job 114366444967](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38104312491/job/114366444967).

Three additional private-bus cases exercise the production Start handler with
an SDK-created session. A directed Response emitted before the Start method
reply is received and decoded; early consent denial returns without reopening;
a mismatched returned request path is refused while the owned path remains
available for cleanup. Together with actual Request.Close dispatch, all four
D-Bus cases passed. Strict Linux Clippy with both portal and owned transport
also passed. This isolates response handling from an installed GNOME dialog;
it does not establish attended native capture or explain a missing live response.
