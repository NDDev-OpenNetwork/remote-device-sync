# Relay TCP test port boundary — 2026-10-10

The relay integration test chose an off-policy target by adding one to the
kernel-assigned echo port. A real run assigned port 65535; debug arithmetic
overflowed before the test could check authorization. This was a fixture
failure, independent of production forwarding or the native chrome change.

The fixture now holds a second live loopback echo listener and uses its
kernel-assigned port as the denied target. Both ports are valid and distinct,
without arithmetic on an ephemeral port. A reachable denied service also makes
the check distinguish policy rejection from an absent TCP listener. The test
asserts the explicit service-refusal response. A policy regression covers the
valid maximum port, a different valid port and reserved port zero.

No production source, dependency, build profile, wire or runtime configuration
changes. The implementation remains the qualified chrome source; this follow-up
repairs its integration verification. The original failing run is retained in
the consuming transaction evidence.

Validation: the complete agent E2E suite passed with default features (17 cases)
and `transport-noq` (18 cases), including the explicit refusal and maximum-port
boundary. Strict E2E Clippy and formatting passed.

Reference: [Tokio TcpListener binding](https://docs.rs/tokio/1.53.1/tokio/net/struct.TcpListener.html#method.bind)
documents port zero as a request for a kernel-assigned available port.
