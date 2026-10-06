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

The final full Mac run passes103net tests and strict workspace desktop/Noq
Clippy after the one-IP and established-candidate guards. Two additional
repetitions of both native restoration fixtures also pass. Linux/CI
and installed qualification remain pending. Earlier fixture failure and
Clippy style errors remain in local evidence. This does not claim complete
physical-network or native pixel latency acceptance.

[QUIC path validation](https://www.rfc-editor.org/rfc/rfc9000.html#section-8.2)
is a prerequisite for using a new path;
[Iroh multipath](https://www.iroh.computer/blog/iroh-0-96-0-the-quic-multipaths-to-1-0)
exposes individual paths and their observations. RDS standby restoration is an
opt-in application policy, not a standard-mandated timer or a new congestion
algorithm. Device/runtime evidence remains private in the estate.
