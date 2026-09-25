# Owned socket failure isolation

Scope: W3.6 local transport isolation and W3.7 prerequisites. This is a
local failure contract, not completed network recovery or a wave-close gate.

## Ownership and error boundary

The Noq endpoint uses one `socket::Mux` over its child transports. Each child
has monotonic retirement state shared by its senders, the receiver and
connection policy. A diagnostic `Health` handle retains metadata and wake
registrations only; it cannot keep socket I/O alive. Each waiting logical
sender, receiver and policy has its own registration, so one connection
cannot overwrite another connection's failure notification.

| Child result | Mux behavior |
|---|---|
| `Pending` | Preserve the child's readiness registration. |
| Send `Interrupted` or `WouldBlock` | Preserve the packet and retry after a 1 ms timer. |
| ICMP/reachability, permission, address, timeout, memory pressure, `EMSGSIZE` or `ENOBUFS` | Count the error without retiring the child; a send is packet loss. |
| Recoverable receive error | Back off that child for 10 ms and continue polling siblings. |
| Other error | Retire that child, wake all observers, stop future I/O polls to it. |
| Every child retired | Return a terminal mux error and explicitly close held connections through policy. |

The timers bound retries; they are not measured service recovery guarantees.
The complete error list lives in `socket/health.rs`. Unknown errors are not
silently retried forever. This classification needs native OS qualification:
an OS error can have wider or narrower scope than an individual packet.
[Linux UDP documentation](https://man7.org/linux/man-pages/man7/udp.7.html)
describes asynchronous errors that can refer to earlier datagrams, and PMTU
errors. Such errors alone do not prove a local socket is permanently unusable.

Receive polling remains round-robin, including after a recoverable error.
The logical socket address and family remain fixed for the engine's lifetime.
IPv4-mapped addresses normalize consistently at the mux boundary.

## Routing and policy

Explicit source IPs use an exact bound source, then a same-family wildcard.
They never fall through to an unrelated specifically bound source. A
source-less route retains its original first matching child, including after
retirement; blindly moving packets to another source/port would skip path
validation. Synthetic relay destinations use the relay child exclusively.
Packets for absent or retired routes are dropped while other transports live.

`bind_endpoint` and `bind_with_mux` wire health into endpoint and policy state.
`bind_with_mux` requires advertised bind addresses to identify live children.
The older `bind_with_socket` accepts an opaque trait object and cannot inspect
its child health; callers needing this integration must use the mux seam.
`transport_health()` distinguishes these cases with `Option`.

On observed retirement, policy withdraws the child's QNT advertisements,
removes unusable pending candidates and demotes/closes observed validated
paths using that child. A failed path is excluded from selection even if the
engine refuses to close its last path. A validated healthy sibling can carry
the existing streams and new datagrams. No sibling means no selected-path
estimate; an unrelated live socket alone does not establish peer reachability.

`addr()` filters failed transports. `local_addr()` and `local_addrs()` remain
bind identities, not health assertions. New dials filter local families and
reject an endpoint with every child retired. Policy admission also checks
terminal health after the handshake. Last-child loss closes held connections
and releases their policy tasks without requiring the application to call
endpoint close first. These checks do not make health, advertisements and I/O
one atomic snapshot; a concurrent failure is handled by the subscribed policy.

## Evidence and remaining work

The [receipt](reports/rds-mux-isolation-20260925.md) covers injected send and
receive failures around real loopback QUIC, continuation of an already open
stream, datagram exchange, QNT withdrawal, fresh connection admission on the
surviving transport, and terminal shutdown with handles retained. Separate
poll-level tests cover independent wakers, error pressure, exact packet retry,
round-robin progress, mapped/source routing and metadata-only ownership.

Still open:

- Full validated path reconciliation after missed engine events. Retirement
  can act only on paths known to policy; see [telemetry coverage](path-telemetry.md).
- Socket recreation, hot interface changes, explicit interface/port routing
  and automatic reconnect. Noq's outgoing source selector lacks a local port;
  multiple same-IP binds cannot represent independent routes through it.
- Relay bootstrap independence, warm relay replacement and UDP-blocked fallback.
  Relay tunnel availability still has its separate existing health contract.
- Global send fairness under a pending child, adaptive retry/pressure policy,
  PMTU qualification, service-specific recovery and weak sampler ownership.
- Native macOS execution, real NIC/suspend/NAT failures, impairment, soak and
  latency/resource comparisons. The injected tests do not establish these.

No dependency, runtime helper process, protocol version or default backend is
changed. All remediation waves remain open.
