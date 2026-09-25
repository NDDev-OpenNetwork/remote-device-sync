# Exact ALPN selection in the owned transport

An owned endpoint accepts at most 16 configured ALPN identifiers. Each outgoing
`connect(target, alpn)` chooses an immutable client TLS configuration offering
exactly that identifier. Locally unconfigured identifiers fail before dialing;
if the peer does not support the requested identifier, TLS fails before the
connection is exposed to application services. There is no fallback to another
configured protocol.

Configurations are built once at endpoint binding and shared across clones.
Concurrent requests for different protocols never mutate the raw endpoint's
default client configuration. Every candidate in a race uses the same requested
protocol and the same expected Ed25519 peer identity. The first authenticated
handshake wins; no service data is sent on losing handshakes.

The configurations share the original endpoint-wide cache bound of 256 TLS
sessions. They use the same certificate resolver and peer verifier, as required
by rustls for sharing resumption state; rustls still checks selected ALPN against
the offer for each handshake. This does not add early application data: the RDS
connect path awaits completion of the handshake and does not enter `into_0rtt`.
Existing underlying TLS early-data settings are unchanged. These tests do not
claim to measure or prove successful session resumption.

## Evidence and limits

Three real-QUIC tests failed before the correction: server preference replaced
the requested second protocol, a missing requested protocol silently fell back,
and concurrent requests both negotiated the first protocol. They now cover
repeated alternating requests on the same endpoints with actual datagrams,
explicit unsupported-protocol refusal and concurrent independent offers.
An additional candidate with invalid port zero fails synchronously before a
valid candidate connects; that failure cannot change the requested protocol.
Existing owned/iroh interoperability checks retain the default `rds/0` behavior.

This is only the ALPN boundary of W2.2. Service capability and limit negotiation,
version compatibility policy, stable session/transfer routing IDs and mixed
service admission remain separate work. No wire identifier or version changed;
no backend was promoted and no remediation wave is closed.
