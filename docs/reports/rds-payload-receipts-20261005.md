# Validated payload receipts — 2026-10-05

W6.8/WS5 delivery follow-up. Waiting on a transport FIN acknowledgement can hold
capture admission after the peer already read a frame. DesktopV4 adds explicit
complete-payload proofs, bound to the session, sequence and BLAKE3 body digest.
The existing three-frame limit remains. Obsolete proofs preserve their separate
disposition; unknown, mismatched, duplicate and canceled registrations cannot
become fresh delivery. Digest contents are excluded from Debug and diagnostics.

The wire reader emits proofs before ordered decode/IPC output. A bounded queue
uses the existing independent control writer with fair input selection. New
combined/separated v5 commands explicitly request this mode. The remote must
return DesktopV4 acknowledgement; legacy acceptance cannot silently downgrade it.
Existing APIs and unrequested modes retain prior behavior and wire discriminants.

Local expanded desktop units and the complete session-v2 fixture suite passed.
New real-QUIC regressions withhold a payload proof despite a consumed stream FIN,
reject unknown sequences/wrong digests, verify a correct proof releases capture,
and keep heartbeat working. Another admitted reader proves its EOF while the
one-entry encoded output queue is blocked. Strict workspace/native-feature clippy
passed. Managed Iroh/Noq tests passed in both legacy and receipt modes, including
five real frames, input/heartbeat events and reuse of the shared connection after
desktop closure. Explicit legacy-ack refusal and route-claim cleanup passed.
DesktopV4 service/grant refusals passed on both Iroh and Noq. Installed
two-device latency/stability qualification remains required.

No hardware, physical scanout or WAN stability gate is closed by these fixtures.
