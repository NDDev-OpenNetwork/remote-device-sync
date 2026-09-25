# TCP destinations and authorization ordering — 2026-09-25

Scope: W2.1 configuration consistency and W1.2 admission ordering. Base `b2e69ff`. Synthetic Linux x86_64
fixtures; no deployment or wave closure.

## Confirmed defects and implementation

Two new binary argument tests failed before the correction: malformed SSH/TCP
destinations survived argument parsing, and agent/client IPv6 parsing differed.
Two policy regressions also failed: equivalent IPv6 spellings did not match,
and development `allow_any_tcp` accepted malformed destinations.

`rds-core::TcpTarget` now owns bounded parsing and canonicalization without DNS
or I/O. Both executable frontends, the library client and the agent wire handler
use it. IPv6 literals require brackets in combined `host:port` flags; separate
library host arguments have no brackets. IPv4-mapped literals canonicalize to
IPv4, ASCII hostnames lowercase, and a final DNS root dot stays explicit. Ports
must be nonzero; ambiguous numeric addresses, URL/userinfo syntax, malformed or
oversized names and non-unicast literal destinations fail before dialing.

The agent compares normalized policy and request targets, then dials that same
normalized request. A different port or address remains denied. Existing public
policy tuple fields stay compatible; their entries are validated during matching.
No wire tags, protocol version, dependency, unsafe code or helper program changed.

The first all-feature validation run also exposed an admission race in the
existing managed-revocation test. The peer could receive a successful Authz ACK
and immediately send Ping while the server still held `Authorizing`, before its
reply task resumed to commit. A deterministic real-QUIC regression held that
window open and failed with the same premature `grant required` rejection.

Services now subscribe before checking admission and wait, with a deadline, only
while an authorization transaction is active. Commit, failed commit and closure
wake the waiters. Each waiter rechecks the committed scope and live revocation
policy; it never uses the provisional grant. Requests before Authz starts still
fail immediately. Reply failure/cancellation still closes the connection and
releases the reservation. Tests cover successful commit, abort and revocation
during the wait. The original managed-policy fixture is unchanged.

## Coverage and checks

Core tests cover canonical roundtrips, IPv4-mapped IPv6, maximum DNS name/label
dimensions, and malformed host/port/address classes. Actual binary subprocesses
require invalid targets to fail before creating an identity file. A real IPv6
TCP echo fixture runs over both iroh and owned noq loopback QUIC connections,
checks off-policy denial, sends malformed wire requests past the client validator,
and proves a subsequent allowed request still works.

Formatting and default/X11/all-feature Clippy passed with warnings denied.
The final workspace run passed **296 tests across 53 targets**,
with one previously qualified migration capacity case intentionally ignored.
The all-feature network/agent/CLI/relay run passed **81 tests across
24 targets**, and feature-isolated binary preflight passed **7 tests**.
[Machine-readable evidence](rds-tcp-target-20260925-data.json) records commands,
exit codes, timings, counts and source hashes.
Full logs stay in private evidence storage. Initial validation found test-module
placement and redundant-qualification lint errors; these were corrected before
the final matrix. `cargo-deny` is not installed; no wave checkpoint is claimed.

## Limits

Canonical syntax is not DNS answer pinning or SSH host-key/authentication policy.
Scoped IPv6 remains unsupported and IDNs require punycode. The accepted local
underscore aliases preserve local resolver use. Native Rust SSH, role/authority
configuration, session negotiation and consistent timeout classes remain open.
Native macOS, real WAN/NAT, deployed SSH/desktop and production acceptance were
not qualified by these local tests.
