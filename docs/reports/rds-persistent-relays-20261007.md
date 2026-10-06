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
requires three live paths, then shuts down the primary and secondary servers.
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

The final increment additionally logs readiness count transitions. Exact final
source repetition, macOS arm64 validation, broader workspace checks and CI are
pending. Earlier candidates failed compilation, address publication, close
withdrawal and standby-path preconditions; their receipts were retained. In
particular, a stale ticket containing one relay does not establish three known
end-to-end alternatives. Full peer addresses and validated path evidence are
required; no route is inferred from local configuration alone.

Host roles, service resource limits, port reservations, credentials and actual
rollout receipts belong to the private estate. Physical network failure,
recovery after suspend, native display response and sustained mixed-load/idle
acceptance still need separate current-build evidence.
