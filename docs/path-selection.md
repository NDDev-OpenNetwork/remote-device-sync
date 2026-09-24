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

## Qualification and limits

A real silent-path regression failed before the correction because a newly
created pending path was Available. A second real test waits for validation of
an unadvertised second listener, checks its actual remote address, closes the
primary logical path and verifies datagram delivery through the replacement.
A deterministic simulation adds a silent candidate beside a slow working path
and checks that the pending default RTT does not win application preference.

Tests that open additional paths wait for their actual creation within a bounded
budget: TLS completion can precede receipt of spare peer connection IDs. That
fixture retry is not a production retry guarantee. Production one-shot opens
can still be rejected before credits arrive; owned retry/backoff is further
W3.6 work. Path event lag and peer-created events preceding subscription lack a
complete state-resynchronization API in the current noq surface. This increment
does not establish complete tracking of every path after lost events.

Logical path closure is not physical NIC/NAT failure. Native macOS, interface
changes, relay failure isolation, global resource/churn bounds and service
interruption budgets remain open. Facade metrics still require full path
enumeration and correct selected/relay attribution. No remediation wave or
owned-backend promotion is closed by these checks.
