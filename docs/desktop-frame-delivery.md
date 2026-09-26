# Desktop frame sender contract

Scope: the partial-write and cancellation portion of W6.1/W2.6. This does not
close the codec reference-chain, serving-session ownership or presentation gates.

Each encoded frame has one tagged unidirectional stream. An in-flight keyframe
finishes before a newer frame is selected; a stale delta is abandoned with
RESET. When the producer closes during a send, its final frame finishes.

The payload writer retains one pinned `write_all` future across producer
events. `write_all` may already have accepted a prefix when its other select
branch wins; restarting it from the beginning would duplicate that prefix.
The sender now continues the same operation. A write or FIN failure is a failed
send, including after supersession or producer closure; it cannot report a
successful completed frame.

One 30-second deadline includes obtaining stream credit, writing the tag and
header, and completing the payload. The stream guard sends RESET on error,
timeout, stale abandonment or cancellation of the frame-writing future. Only a
successful FIN disarms the guard. RESET affects that frame, preserving the
shared connection. The deadline is a local failure bound, not a frame-latency
target or a negotiated wire capability.

Regression coverage uses a 64-byte duplex buffer and a 4096-byte patterned
payload to force a partial write before each producer event. Both supersession
and closure previously delivered 4160 bytes; both now preserve exact bytes.
Other tests retain errors after the producer event, abandon stale deltas without
waiting for a reader, and observe RESET after aborting an owned sender on real
iroh/noq connections while a subsequent stream still succeeds.

Remaining work includes cancellation ownership of the whole serving session,
reference-aware queue collapsing with real codec continuity tests, per-session
wire IDs, global encoded/decoded/native allocation accounting, managed viewers,
rendering/input release and platform/network acceptance. See the
[client receive contract](desktop-client-lifecycle.md). No wire format, frame
priority, dependency, installed binary or published release changes here.
