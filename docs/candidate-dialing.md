# Owned initial candidate dialing

The owned noq backend considers up to eight deterministic direct candidates and
one synthetic route through its already attached relay. It starts their QUIC
handshakes concurrently under one 15-second absolute deadline. A synchronous
address error, authentication failure or silent address cannot prevent another
candidate from succeeding. Empty candidate sets fail before dialing.

Every attempt pins the same expected Ed25519 endpoint identity through TLS. The
first fully authenticated handshake wins. No service request, authorization or
policy driver is started on the losers. Pending attempts are aborted and joined;
any already completed losing connection is explicitly closed before returning
the winner. Additional multipath addresses are then offered on that connection.
The selection of those paths is a separate policy, with validation work remaining.

The attempt group belongs to the connect future. Canceling it aborts its owned
attempts; dropping each noq Connecting closes its handshake. QUIC may retain
closing packet state for its protocol-defined three-PTO period. This is not an
instant packet-state deletion guarantee. A real cancellation test retains the
endpoint and observes its engine reach draining, so dropping an endpoint cannot
mask retained attempts. Twelve supplied addresses result in eight attempts.

## Socket-family consistency

The mux reports an IPv6 logical socket whenever an IPv6 child exists, independent
of bind order. At the QUIC boundary IPv4 receives use IPv4-mapped IPv6; before
child sends mapped addresses return to native IPv4, including synthetic relay
routes. Native IPv6 scopes are preserved. Endpoint advertisements retain the
actual bound child addresses, not this internal representation.

Locally unsupported families are filtered before the eight-candidate limit and
before application-driven QNT path opens. Synthetic addresses are not accepted
as public direct candidates. The QUIC engine can also emit QNT probes internally,
before policy sees an address. When no child can carry one of those datagrams,
the mux drops it as an unroutable candidate instead of returning an I/O error
that kills healthy paths. Actual child I/O failures retain their error behavior;
complete failure isolation between transports remains W3.6 work.

## Qualification

Two regressions failed on the old first-address-only implementation: silent
first IPv4 with a healthy second address, and silent direct address with an
already attached relay. Tests also exercise wrong identity alongside a valid
candidate, all candidates silent, IPv4-to-IPv6 fallback, cancellation/candidate
limits, unsupported families preceding the candidate cap, both socket orders
in both connection directions, dual-stack relay routing, and two live addresses
leaving one exposed connection. Actual datagrams
verify the winning route. Loopback deadlines are correctness assertions, not
WAN latency measurements or claims of fastest possible connection time.

## Remaining W3.1/W3.6 work

This increment does not make relay bootstrap independent: endpoint creation
still waits for its configured relay attachment. Interface enumeration, remote
address scope and advertisement filtering, different LANs/NATs, TCP/443 fallback,
relay replacement and validated multipath selection remain open. Limits are per
connect, not global across all callers. Candidate errors currently report the
last failed attempt; per-candidate structured diagnostics remain W2.8. The
[exact ALPN boundary](protocol-negotiation.md) is now checked separately; full
capability/version negotiation remains W2.2 work.
The default backend remains iroh until the parity gate is qualified.
