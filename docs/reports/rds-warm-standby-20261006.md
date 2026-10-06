# Standby restoration and retained IP selection — 2026-10-06

ACK-aware retirement can leave a connection with only one live path if its
previous standby is never opened again. Iroh originally offered known relay
paths only at client connection initialization; ordinary upgrade work targets
IP candidates. The latency opt-in now restores known relay/custom standbys on
its existing periodic refresh, using the original client-only open/validation
API. Noq similarly restores previously established original-ticket candidates
through its bounded pending queue. Permanently rejected cold candidates cannot
be retried forever by this policy; drain, unavailable transport and family
filters remain. No strong connection owner crosses an await.

A real Iroh fixture first retired its custom standby, then verified a new
validated path on the same connection and positive ACK proof. Selecting that
restored custom path exposed another defect: an unchanged map count caused the
loop to close every IP sibling. The failed fixture retained only the custom
path and stranded22reliable bytes after its later blackhole. Selection now
keeps one live IP, preferring an already established local selection and then
lowest RTT. The same fixture subsequently delivered those22bytes on its original
stream after loss. A real Noq fixture consumes its initial candidate offer,
closes an established standby, waits for a different validated path ID and ACK
proof, then exchanges more bytes on the original stream.

The initial local Mac run passed 103 net tests and strict workspace desktop/Noq
Clippy after the one-IP and established-candidate guards. Two additional
repetitions of both native restoration fixtures also pass. Later CI failures superseded that local-only result. Linux/CI
and installed qualification remain pending. Earlier fixture failure and
Clippy style errors remain in local evidence. This does not claim complete
physical-network or native pixel latency acceptance.

[QUIC path validation](https://www.rfc-editor.org/rfc/rfc9000.html#section-8.2)
is a prerequisite for using a new path;
[Iroh multipath](https://www.iroh.computer/blog/iroh-0-96-0-the-quic-multipaths-to-1-0)
exposes individual paths and their observations. RDS standby restoration is an
opt-in application policy, not a standard-mandated timer or a new congestion
algorithm. Device/runtime evidence remains private in the estate.

## Probe-only debt versus retained active-route work

Final44013ae CI/native Linux exposed a later blackhole still removing the IP
standby. A standby probe ACK can travel over the selected path; its absence
alone must not discard the only alternative. Restricting retirement to only the
current selection also stranded bytes sent during a previous active tenure when
RTT ranking switched first, so that experiment is retained as a failed check.

The subsequent active-tenure heuristic passed a local 104-test run but failed
later CI and a stronger data-bearing-IP fixture. That heuristic has been removed;
those passing runs are historical and do not qualify the current source.

## Exact STREAM observation and independent ACK routing — 2026-10-07

The published Noq-proto 1.3 source is vendored with retained licenses and checksum
provenance. A PathStats boolean reads existing sent-packet STREAM metadata.
Retirement now requires that actual pending work, ACK starvation, and an
established sibling with fresh positive ACK evidence. No selection marker cell,
payload parsing or application replay is needed. Stats are sampled once per
candidate per policy evaluation, with opt-in debug records for stream work,
selection, RTT and ACK ages.

An isolated trace exposed a second issue: a standby could have outstanding PING
work and stale ACK proof, which suppressed every later liveness probe. Meanwhile
its PATH_ACK could depend on the selected failing route. Stale confirmation now
requests another probe on the existing bounded selector cadence. Latency mode
also enables a default-false same-path ACK option: a live validated Backup can
carry its own ACK. Abandoned/unvalidated paths retain cross-path ACK fallback;
handshake and single-path behavior are unchanged. This is permitted scheduling
under [Multipath QUIC §5.5](https://www.ietf.org/archive/id/draft-ietf-quic-multipath-21.html#section-5.5),
not a mandatory standard timer or a new congestion/loss algorithm.

The stronger real Iroh fixture transfers and acknowledges data over IP before
making it standby, then recovers 22 existing-stream bytes after the restored
custom route fails. A real Noq fixture observes PING-only=false,
unacknowledged STREAM=true, and acknowledged STREAM=false. Another checks
PATH_ACK counters on the actual Backup carrier while the primary remains
Available, without promoting the Backup or sending STREAM frames there.

The first revised full Mac net run passed 106 unit tests and all integration
suites, including interop, lifecycle, mux isolation and deterministic simulation.
Default and desktop/Noq workspace Clippy passed. Earlier exact-metadata-only
and ACK-routing-without-repeat-probes runs failed the later blackhole and remain
in private check artifacts. Final source/CI, native Linux, installed and physical
pixel-latency qualification are still pending. No failed source is eligible for
deployment; complete native stability is not established by these synthetic tests.
