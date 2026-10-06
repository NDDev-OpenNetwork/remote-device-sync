# Iroh1.3 periodic custom selection patch

Upstream: published crates.io `iroh`1.3.0, checksum
885787b892b5e2507c701f132ecbd45d2bad4bbb75157a19427087c16dadb833.
Original MIT/Apache-2.0 notices and source retained. No QUIC/TLS/relay wire change.

The published selector only runs after connection/path topology events. Add one
optional default-None selector refresh interval, bounded250ms..60s. The RDS
latency selector requests1s; ordinary/pinned selectors preserve original events.
Unchanged selections are not reapplied during refresh. The actor already owns
only weak connection references; refresh does not add connection ownership.
No default behavior change for other selectors. This temporary source patch
keeps the working substrate while the owned Noq backend retains its existing
periodic lowest-RTT policy. Upstream qualification and convergence remain work.
