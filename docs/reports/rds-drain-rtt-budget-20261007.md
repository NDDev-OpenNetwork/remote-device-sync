# Owned drain priority and real RTT budgets — 2026-10-07

Scope: W3.3/W3.6, experimental owned Noq policy; this does not change the default
Iroh profile or close a native/WAN gate.

The selector assigned `Duration::MAX` to a draining relay to prefer a live
alternative during its usable grace period. The same value was also passed to
ACK freshness, stall detection and replacement proof, making those time budgets
effectively infinite and labeling the administrative penalty as measured RTT.

Keep measured RTT and selection rank separate. Drain still receives the maximum
ranking penalty; progress decisions and diagnostic RTT use the actual path
measurement. The engine controller, wire, last-path guard and drain grace remain.

The regression drives the real passive Cubic adapter's ACK-eliciting send
callback with a controlled runtime clock. Three seconds of pending work on a
70 ms path must be stalled even while that path has maximum drain priority.
It failed with the former conflated value and passes with separate values.
All 111 Mac net/Noq library tests pass, including actual original-stream
blackhole recovery, standby restoration and independent backup ACK routing.
Strict all-target Noq Clippy and broader/platform qualification are being
collected. No installed owned-backend improvement is asserted from these tests.

[RFC 9002 §5](https://www.rfc-editor.org/rfc/rfc9002.html#section-5) defines RTT
estimation from transport timing. An administrative selection penalty is not
an RTT sample. RDS's freshness/retirement thresholds remain application policy;
separating their inputs is not a new congestion or loss-detection algorithm.
