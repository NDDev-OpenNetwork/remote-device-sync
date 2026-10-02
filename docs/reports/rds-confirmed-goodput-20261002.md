# Preserve confirmed timely media goodput

Scope: W6.4/W6.7 adaptive media and diagnostic remediation. Native physical
latency, quality and sustained stability acceptance remains open.

Repeated packet-loss samples could multiply the offered bitrate down to its
floor even while recent media receipts established higher timely delivery.
Reliable QUIC packet loss and actual media goodput measure different things.
The increment credits payload bytes only after successful normal transport
receipt and only when full stream open/write/receipt duration stays inside the
existing delivery-delay budget. Peer disposal, unknown stops, timeouts and late
successes do not contribute. Pacing before opening the stream is still included
in the aggregate wall-time goodput window; the rate does not subtract that wait.

Two adjacent one-second windows each need at least three timely receipts and
4096 bytes. The smaller measured rate, reduced by 20%, can bound a loss-only
cut. It never exceeds the negotiated ceiling. No ACK, media pressure, delivery
holds, producer misses or suspected/sustained RTT growth retain their original
responses. Unknown/changed path identity, outstanding late receipts, failure,
silence or insufficient traffic clears the observation. This is not total
network capacity, codec correctness or physical display timing.

INFO health adds timely byte/receipt counts and the eligible delivery floor.
These are metadata only. No protocol/identity/authorization format changes.

## Regression evidence

- A seeded 15% packet-loss replay with timely confirmed goodput keeps at least
  that conservative observed floor. The equivalent original response without
  the evidence falls below it; the fixture distinguishes both behaviors.
- Failure, missing delivery, producer pressure and RTT growth cannot use an
  asserted goodput floor to suppress their cuts. Negotiated ceilings hold.
- Explicit-clock window fixtures cover two-window admission, unequal rates,
  idle/sparse traffic, path changes, missing selection and expired evidence.
- Receipt classification gives no timely-byte credit to a late completion;
  real Iroh/Noq obsolete/unknown-stop regressions additionally check zero credit.
- The admission-cadence fixture exposed by the prior macOS CI is repaired with
  the production resume/advance functions on an explicit clock. It no longer
  assumes the OS schedules two real-time calls within one 16 ms frame slot.

The combined macOS run initially retained an independent v1 sync failure:
`control stream ended`. Its isolated investigation passed. The negotiated-sync
pair had default public discovery/port mapping; it is now explicitly local UDP
with discovery disabled, and its failure reports both client and server reasons.
No accepted operation is retried and no failure assertion is removed. The
original negative log remains retained. Final platform and public CI results
are recorded with the candidate PR; incomplete lanes are never called green.

```sh
cargo fmt --check
cargo test --locked -p rds-desktop -p rds-client -p rds-cli -p rds-sync --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```
