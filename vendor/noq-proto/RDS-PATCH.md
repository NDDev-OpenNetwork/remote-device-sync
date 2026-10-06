# Noq-proto 1.3 path observation and optional ACK routing

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
