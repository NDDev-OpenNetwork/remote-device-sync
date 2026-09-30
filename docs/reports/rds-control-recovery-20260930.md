# Control priority and retry pause regression — 2026-09-30

This follow-up advances W6.7 control scheduling and W6.3 session recovery. It
does not close a remediation checkpoint or a physical latency/soak gate.

Previously only the serving desktop control half had high QUIC priority.
Ping/Info/authorization/renewal and outgoing desktop input/heartbeat streams
retained the default below media. The shared wire helper now applies the control
class before the first write at both request and reply boundaries. Desktop
client/server control uses the same highest class; media uses the shared middle
class. Streaming TCP/sync bodies are not promoted by their greeting. Wire and
local IPC formats are unchanged.

The old reconnect pause selected once between its timer and input. Any discarded
pointer/key event ended the pause and triggered another connection attempt. It
now retains one timer while draining input, with immediate close/cancellation.

Two real owned-QUIC regressions queue a 256 KiB media-class body and suppress
peer ACKs to expose scheduling across an exhausted congestion window. Ordinary
Ping and Info exercise the actual client request and agent reply implementation
in each direction. Both tests failed on the preceding behavior and passed after
the priority fix. The socket gate is synthetic, loopback-only and bounded; it
does not stand in for WAN, loss, grant renewal or full mixed-load qualification.

A virtual-time retry regression feeds input every second during an eight-second
pause. The preceding behavior returned on the first event; the fix waits exactly
eight seconds, drains input and does not restart its timer. Separate assertions
verify immediate close/cancellation. These checks use no ambient GUI or input.

Validation receipts for the exact source and installed-device observations are
recorded at their owning boundaries. This scoped report makes no new latency SLO,
hardware-backend, monitor/IME or long-duration availability claim.
