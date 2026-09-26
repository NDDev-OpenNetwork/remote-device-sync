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

Historical reports named `transfer` ended at sender finish and did not prove
receiver completion. They are retained as historical evidence, but their rates
must not be used as a receiver-goodput baseline. The versioned scenario name
makes the existing comparator reject a missing matching scenario rather than
compare these incompatible measurement methods.

W0.2 remains partial: known-rate path calibration, per-phase connection and
authorization timings, full topology coverage and shared-link load measurements
are separate qualification work. Same-host results do not establish WAN capacity
or user-visible desktop latency.
