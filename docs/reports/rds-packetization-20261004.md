# Packetization qualification controls — 2026-10-04

W1/W6 follow-up. This increment exposes existing path-kind restrictions in
endpoint files and adds an opt-in conservative primary QUIC packetization
policy: fixed 1200-byte UDP payloads with MTU discovery/GSO disabled. Adaptive
defaults, congestion control, stream budgets, identity and authorization remain.

Strict file/preflight tests cover unknown policies, relay requirements,
incompatible direct-only configuration, roundtrip and preservation across flag
overrides. Default serialization omits the new fields for the legacy shape.
Older strict binaries require an upgrade before explicit new fields are used.

Real loopback Iroh/Noq endpoints send and echo 256 KiB through one UDP proxy.
Receiver bytes must match; proxy byte counts must cover both payload directions;
the actual maximum UDP datagram must be at most 1200 bytes. Noq packetization
configures the primary connection; an independently attached relay's outer
transport is not covered by this byte-size claim.

Local feature-complete workspace check and the 86 network unit tests pass, as
do both wire-level packetization cases. Final published checks and real-device
comparisons remain separate. This is a qualification mechanism, not a closed
latency/stability gate or a universal recommendation to disable offload.

The native diagnostic comparison also exposed a cadence defect: a ready
damage producer could bypass negotiated max_fps. Session admission now enforces
the minimum capture-start interval regardless of platform idle waits. Capture
cost consumes that interval, avoiding an extra frame interval after encoding.
The bounded cancellation check remains active while waiting. A real loopback
regression with an immediate producer failed the 5 FPS wire-timestamp limit
before the change; the repaired producer must space its five observed frames
by at least 199 ms (one millisecond timestamp quantization allowance).
