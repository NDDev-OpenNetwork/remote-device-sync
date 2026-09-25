# Validated path eligibility in the owned backend

The application policy initially seeds only PathId::ZERO from a completed,
authenticated handshake. It subscribes to path/QNT events before its explicit
advertisements, traversal request and additional-path opens. Pending PathIds
returned by an open attempt are not validation evidence.

Additional direct, attached-relay and QNT-learned paths open as Backup. The policy
adds a weak path handle to its eligible set only after Established. It then uses
the existing RTT selection with a 5 ms switching margin among eligible paths.
Abandoned/discarded paths are removed; learned candidates already eligible or
already pending do not repeatedly increase pending history or attempt counters.
A known validated backup can become preferred after the selected path closes.

The underlying noq engine already prohibits application data on unvalidated
paths. This change aligns application priority, selection and hysteresis state
with validation; it does not claim the old engine sent payload on unvalidated
routes. In particular, a pending path's initial RTT estimate is not a measurement
that can justify demoting a slower but working path.

Task ownership remains weak. No Connection, Path or OpenPath handle is held by
the policy across an await. Dropping the last application I/O handle can still
close the connection, while explicit endpoint close waits for policy tasks.
Endpoint shutdown seals admission and signals every owned policy future. Each
future directly closes its weakly referenced connection before exiting, even if
a fatal sender I/O error has already stopped noq's protocol driver. Merely queuing
an endpoint-close event to that stopped driver is insufficient when multiple I/O
handles remain. The signal also releases pending candidate retries. Subscriptions
are still created before spawning, and no strong handle crosses an await.

## Qualification and limits

A real silent-path regression failed before the correction because a newly
created pending path was Available. A second real test waits for validation of
an unadvertised second listener, checks its actual remote address, closes the
primary logical path and verifies datagram delivery through the replacement.
A deterministic simulation adds a silent candidate beside a slow working path
and checks that the pending default RTT does not win application preference.

TLS completion can precede receipt of spare peer connection IDs. Production
candidate opens now use the bounded queue described below; test helpers that
explicitly call `open_extra_paths` remain one-shot. Path event lag and peer-created
events preceding subscription lack a complete state-resynchronization API in the
current noq surface. Candidate-address reconciliation is not path-validation
reconciliation and does not establish complete tracking after lost path events.

Logical path closure is not physical NIC/NAT failure. Native macOS, interface
changes, relay failure isolation, global resource/churn bounds and service
interruption budgets remain open. Facade metrics still require full path
enumeration and correct selected/relay attribution. No remediation wave or
owned-backend promotion is closed by these checks.

## Temporary path-credit exhaustion

The existing policy task owns pending initial direct/attached-relay candidates
and QNT advertisements. `RemoteCidsExhausted` and `MaxPathIdReached` defer path
allocation instead of silently losing the candidate. Each queued address has a
15-second absolute lifetime, retries after 25, 50, 100, 200 and then at most
400 ms, and is removed after allocation or permanent rejection. There are at
most 41 queued addresses (eight initial direct, one relay, 32 advertisements),
with at most eight due opens per loop iteration. Idle queues have no retry timer.

Mapped IPv4 addresses share one entry with their native representation. Duplicate
queued advertisements do not reset backoff or extend the original lifetime. A
withdrawal removes a queued advertisement but preserves an independently supplied
ticket address. Subscription precedes the initial advertisement snapshot; a
lagged QNT address stream triggers another bounded snapshot. Withdrawal does not
close an already allocated path. Expiry limits one queue admission; a subsequent
new advertisement can admit the address again, so this is not a connection-wide
or peer-wide churn budget.

Allocation still does not imply validation: only Established makes an extra path
eligible for selection. No retry owns a strong connection across an await or
spawns another task. A 200 ms one-way simulation asserts immediate typed credit
exhaustion, automatic later validation of an unadvertised second listener, and
datagram delivery after logical primary close. Before the fix, automatic opening
failed while the same manual open succeeded once credits arrived. A companion
simulation drops the last connection during backoff and checks that its policy
task exits while the endpoint remains alive.

This queue retries local allocation refusal, not failed path validation, session
reconnection or application requests. It does not provide global connection
admission, reconnect jitter, transport-failure isolation, complete path event
recovery or real-network performance qualification.

## Shutdown after a protocol-driver I/O failure

A real UDP test first carries an authenticated datagram, then injects a terminal
send error and confirms that the sender returned it. Both a Connection and Path
handle remain alive while endpoint close runs. Before the shutdown signal was
added, close exceeded the one-second test deadline; direct Connection::close was
needed for fixture cleanup. Afterward close completes, the retained connection
has a close reason and the endpoint owns zero policy tasks. An earlier single-
handle fixture completed through implicit close and did not reproduce this case.

This is an explicit local shutdown guarantee for admitted connections, not
recovery of a failed protocol driver, generic child-socket isolation, successful
peer notification, or proof that every underlying QUIC packet/history entry has
drained. In-flight handshakes and service/relay task groups retain their separate
ownership and timeout contracts. Connection-state mutex work and OS/runtime
scheduling are not given a hard real-time bound by this test.
