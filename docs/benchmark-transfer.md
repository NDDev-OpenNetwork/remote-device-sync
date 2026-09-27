# Verified transfer measurement

The `rds-bench run --scenario transfer` command reports
`transfer-receiver-ack-v1`. It measures application goodput over an established
QUIC connection and an agent-forwarded loopback TCP stream. It is development
tooling, not a service exposed by the installed agent.

The receiver streams the body through BLAKE3 using a 64 KiB buffer. Only after
payload EOF does it return a fixed 40-byte receipt: an eight-byte big-endian
byte count followed by the 32-byte digest, then response EOF. The sender checks
all three. Short/corrupt payloads, incorrect/truncated receipts, trailing data,
I/O errors and timeouts produce no successful throughput result.

The sender generates deterministic position-dependent data with the existing
BLAKE3 XOF, using the fixed `rds-bench-transfer/v1` seed. Its timed interval
includes generation, sender hashing, upload, receiver hashing, the receipt and
response EOF. Connect and OpenTcp setup are excluded from goodput. This is not
raw transport capacity, filesystem durability, or a synchronization benchmark.
The fixture keeps at most eight receiver tasks with bounded per-task buffers;
the world's owned task group cancels them when the scenario ends or fails.

`--timeout-s` bounds world startup separately, then one combined deadline
covers connect, OpenTcp, upload and receipt. Endpoint cleanup has a separate
five-second deadline. Zero/overflowing payload sizes and zero timeouts fail
before startup. Reports include `transfer_verified_bytes` and
`transfer_completion_ns` as well as MiB/s.

Since W0.2 the report also carries a phase split: `phase_connect_ns`
(endpoint connect incl. handshake) and `phase_service_open_ns` (OpenTcp —
service grant + forward setup) are measured separately and never folded
into goodput. `resolve-connect` reports per-phase percentile series
(`phase_resolve_*`, `phase_connect_*`, `phase_first_byte_*`) so the G3
budget can be attributed; `ping` and `migration` record `phase_connect_ns`.

`--scenario calibration` reuses the transfer body over a rate-capped
direct path (`--rate-mbps`, default 10 Mbps — deliberately below the
loopback ceiling so the cap binds). The measured verified goodput must
land within 0.4×–1.2× of the cap in bytes/s or the lane fails: below the
floor means the transport under-performs the imposed ceiling, above means
the limiter or the measurement is fabricating throughput. The report
records `calibration_expected_bytes_s`, `calibration_measured_bytes_s`
and `calibration_ratio_milli`.

`--scenario recovery` (noq only) starts a verified upload on a clean
socket-impaired path, then at ~⅓ payload imposes a ~1.5 s loss+delay
burst on the client's live socket (loss floored at 15%) and lifts it.
The verified receipt must still arrive and the drop counter must be
nonzero — the scenario fails if loss never engaged or the transfer
stalls past the deadline. iroh reports SKIPPED: its impairment is a
static spawn-time proxy leg.

Historical reports named `transfer` ended at sender finish and did not prove
receiver completion. They are retained as historical evidence, but their rates
must not be used as a receiver-goodput baseline. The versioned scenario name
makes the existing comparator reject a missing matching scenario rather than
compare these incompatible measurement methods.

W0.2 remains partial only on breadth: full topology coverage and
shared-link load measurements
are separate qualification work. Same-host results do not establish WAN capacity
or user-visible desktop latency.
