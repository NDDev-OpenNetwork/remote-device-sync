# Noq-proto 1.3 path observation and optional ACK routing

`Connection::established_paths` now exposes a read-only snapshot of path IDs
whose initial establishment is informed and which have not been abandoned.
It includes the handshake path while the connection is open. The Noq adapter
forwards this for Iroh's subscribe-then-snapshot registration; no validation or
packet scheduling decision changes. A pending/abandoned/high-ID/closed-connection
regression covers the snapshot boundary.

Exact published crates.io noq-proto1.3.0 source, checksum
7c1e5b6fe668491eca022f745a0a9402585626c73a7b839b3424ace15d6a9c8f.
Published upstream revision: c1f411562e6078852749b8bcf1190523096a107f,
subdirectory noq-proto. Original MIT/Apache-2.0 license files are retained.
One additive PathStats bool
reports whether the path packet-number space retains unacknowledged STREAM
metadata. It reads existing sent-packet bookkeeping, exposes no application
bytes. The observation changes no wire, crypto, loss, pacing or retransmission algorithm.
RDS uses it to distinguish reliable stream work from PING-only debt before
path retirement.

TransportConfig::prefer_same_path_acks is a separate default-false opt-in.
When multipath is negotiated, ACKs for a validated, live path prefer that
path, including when it is Backup. ACKs for abandoned/unvalidated paths
can still travel on another path; handshake and single-path scheduling are
unchanged. RDS enables the option only for PathPreference::Latency, through
both Noq and the narrow Iroh builder forwarding method. No new wire frame,
crypto, loss detector or congestion controller is introduced. This avoids
making backup ACK delivery depend on the failed selected path.

Policy guidance: https://www.ietf.org/archive/id/draft-ietf-quic-multipath-21.html#section-5.5
permits same-path ACK scheduling; this is an implementation choice, not an
IETF-mandated default. Tests in rds-net's real Noq loss-socket fixtures verify
the ACK carrier, probe/STREAM distinction and clearing after ACK. The real
Iroh fixture restores a retired standby, transfers and drains IP work, then
recovers existing stream bytes after a later custom-link blackhole.
Upstream contribution/convergence and installed qualification remain work.

The MTU constructor's debug assertion keeps its numeric invariant but uses a
static message. CodeQL treated the formatted minimum-MTU value as configuration
data derived from test certificate setup. Removing the interpolation avoids
that diagnostic data flow without changing the MTU decision or disabling scans.

## Initial multipath validation deadline

A new unvalidated path now has a dedicated deadline independent of its idle
policy. The budget is three times the larger initial PTO and PTO of the live
validated paths. This conservative choice follows RFC 9000 §8.2.4 guidance;
using all live validated estimates avoids judging an unknown route solely from
an unrelated faster path. RFC 9000 migration retains its separate timer and
previous-route fallback. Validation success cancels the initial timer. Failure
closes the attempt with the existing TimedOut reason and PATH_ABANDON machinery;
last-path protection, CID retirement, ID nonreuse and retransmission remain in
the existing engine boundary. Qlog records the standard path-validation timer.

No new wire frame, credential, congestion controller or application replay is
introduced. Multipath draft-21 §3.1 requires explicitly closing a failed path
initiation; §3.4 prohibits reusing an abandoned path ID. Deterministic engine
regressions cover no-idle-policy timeout, fresh-ID recovery and last-path
protection. The complete engine unit suite runs on both supported CI targets.
A real Iroh actor fixture blocks an initially configured standby, restores it
and continues the original bidirectional stream through the same connection.

Sources: https://www.rfc-editor.org/rfc/rfc9000.html#section-8.2.4
and https://www.ietf.org/archive/id/draft-ietf-quic-multipath-21.html#section-3.1
