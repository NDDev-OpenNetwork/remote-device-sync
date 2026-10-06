# Bounded persistent relay registrations — 2026-10-07

This increment advances v0.4 multi-relay readiness; C2/G2 and installed WAN/native
latency qualification remain open. No production rollout is asserted here.

## Reproduction

`cargo test --locked -p rds-net --test relay_registrations -- --nocapture`

Three actual Iroh relay servers bind independent ephemeral loopback listeners.
Both endpoints have only relay transports, explicit latency preference, three
persistent custom registrations and configured-order relay preference. After
65 seconds with no peer session, the fixture requires three advertised ready
addresses. It opens one bidirectional stream using the complete peer address,
requires three live paths and confirms the expected selected URL at both peers
before shutting down the primary and secondary servers.
Each fault must change the selected path and preserve the same connection and
stream; the exact response byte must match. The five-second deadline is a test
bound, not a WAN SLA. Failed registrations must disappear from both addresses.

The administrative fixture removes a configured origin, requires withdrawal
within two seconds and verifies dynamic insertion cannot expand the initial
persistent-address set. Explicit endpoint close seals readiness synchronously.
The default fixture retains one home relay and checks opt-in validation bounds.

## Evidence and limits

The Linux x86_64 development candidate passed all three real-relay fixtures,
configuration tests and strict all-target `rds-net` Clippy with `transport-noq`.
Its archive SHA256 was
`b07cc638b734b6c897f007bdfbe5f557e02de3456925d2743bc3554024d4ec76`.
Selected-primary failure responded on the original stream in 743 ms; the next
selected tier in 952 ms. These are two loopback samples, not percentiles,
throughput measurements or physical input-to-pixel evidence.

Final implementation source `e5b7c990579ec13d2ebb7f8bf2d216bcce485a2c`
passed the complete CI matrix on Ubuntu and macOS, native builds, supply-chain
and Rust/Actions CodeQL checks: 16 successful checks and two standard skips.
The release job's synthetic merge `1721f38eaddd4e466e29f2b946312966642b53d2`
has the exact same tree `a921443927de61cca21ac026228ba7386dfc7896`.

The final Linux source archive SHA256 was
`bbf1bb724a438ec4553c0329db9f9184383ac80f5a77670939a02ea6ea34f097`.
Its three consecutive selected-relay fault fixtures passed (ordinary fixture,
explicit repeat and the complete net/Noq suite), along with all 108 net unit
tests, integrations, strict workspace desktop/Noq Clippy and the Linux X11 lane.
First/repeat fault responses were 743/1028 ms and 1086/997 ms. Mac's corrected
selected-URL fixture passed in 1091/963 ms; strict workspace desktop/Noq Clippy
passed. The remaining unused helper in that development test was removed in
final source and CI qualified the resulting test. No failure was suppressed.

Earlier candidates failed compilation, address publication, close withdrawal
and fault preconditions; their receipts were retained. A fixture with three live
paths could still fault an unused relay before selection converged. The final
fixture fences on the selected URL at both ends rather than relaxing recovery.
A stale ticket containing one relay also does not establish three known
end-to-end alternatives. Full peer addresses and validated path evidence are
required; no route is inferred from local configuration alone.

Readiness count transitions are metadata-only logs. Explicit endpoint close
seals publication before returning; transport draining retains its original
ownership. This report qualifies the implementation increment, not C2/G2 or
installed native/WAN acceptance.

Host roles, service resource limits, port reservations, credentials and actual
rollout receipts belong to the private estate. Physical network failure,
recovery after suspend, native display response and sustained mixed-load/idle
acceptance still need separate current-build evidence.
