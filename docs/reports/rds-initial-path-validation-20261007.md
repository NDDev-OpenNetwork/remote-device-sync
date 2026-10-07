# Initial multipath validation — 2026-10-07

A newly allocated multipath path could remain unvalidated indefinitely when
its established-path idle policy was disabled. `open_path_ensure` continued to
return that same unvalidated attempt, retaining its challenge backoff. Initial
path creation did not arm an independent validation deadline; the existing
validation timer belonged to RFC 9000 address migration.

## Correction and boundaries

Add a dedicated initial validation timer using three times the larger initial
PTO and PTO of live validated paths. Successful validation cancels it. An expired
unvalidated attempt closes through existing PATH_ABANDON, CID retirement and
fresh-ID allocation; the last remaining path is protected. Migration keeps its
separate timer and previous-route fallback. The shared vendored Noq-proto engine
serves both Iroh and owned Noq. Qlog uses the path-validation timer category.
No wire frame, application replay or congestion controller is added.

[RFC 9000 §8.2.4](https://www.rfc-editor.org/rfc/rfc9000.html#section-8.2.4)
recommends a validation timeout based on three PTOs and the larger new/current
estimate. Using all live validated estimates is this implementation's
conservative extension for multiple known paths, not a mandated exact formula.
[Multipath draft-21 §3.1](https://www.ietf.org/archive/id/draft-ietf-quic-multipath-21.html#section-3.1)
requires explicit closure of a failed initiation;
[§3.4](https://www.ietf.org/archive/id/draft-ietf-quic-multipath-21.html#section-3.4)
prohibits reusing its consumed ID. These standards predate the requested
2026-09-26 research boundary; source and verification observations are dated
2026-10-07.

## Retained verification

- The new deterministic no-idle-policy regression failed on the original engine:
  no abandonment event arrived after the validation interval. It passes with the
  correction, including fresh-ID restoration and surviving established sibling.
- A first full engine candidate passed 427 tests and failed one historical
  client/server event-order expectation. Separate initial/migration timers and
  an assertion accepting either independently expired or peer-abandoned closure
  of the same failed ID retain the actual invariant. The final engine suite has
  429 passing tests, including last-path protection.
- macOS: qlog compilation, the complete default/Noq network tests and strict
  all-target network Clippy passed. A real Iroh actor with an initially blocked
  custom standby restored it in 1006ms and continued the original stream.
- A bounded loopback TCP gate accepts the standby socket while delaying the
  authenticated relay protocol on one side. The other endpoint is already
  registered. After restoring the gate, the existing connection validates the
  standby in 987ms and retains the original bidirectional stream. Gate tasks and
  sockets remain owned by the fixture; there are no external hosts.
- Both real actor/gated-relay fixtures also pass on the original production
  source. They establish preserved behavior and an acceptance bound, **not** a
  causal performance improvement from this correction. An observed installed
  recovery delay is not explained by these loopback results.

The full engine suite now runs in both supported CI test lanes. Linux/X11,
current-head CI, immutable release builds and installed qualification remain
required before rollout. This report does not close W3/W6/W10 or claim physical
interface, suspend, power-loss or causal input-to-pixel acceptance.

## Measurement harness increment

Current-source `rds-bench` measurements are retained in
[rds-initial-validation-bench-20261007.json](rds-initial-validation-bench-20261007.json).
Source: `378d636a93548e7a1b15c1b1c45bb1749619a9ea`, macOS arm64,
`transport-noq`, unoptimized stripped development profile, one run per case.
Synthetic same-host direct endpoints use temporary identities. No deployed host,
private configuration or foreign desktop is part of these measurements.

| Case | Samples or verified payload | Observed result |
|---|---|---|
| Iroh ping | 32 | p50 0.558ms; p95 0.765ms; p99/max 0.962ms |
| Noq ping | 32 | p50 0.428ms; p95 1.669ms; p99/max 1.859ms |
| Iroh receiver-verified transfer | 4MiB | 18.163MiB/s |
| Noq receiver-verified transfer | 4MiB | 25.547MiB/s |
| Noq controlled mid-transfer loss | 2MiB verified | 179 dropped datagrams; verified receipt; 1.770MiB/s |

Transfer includes bounded streaming generation/hashing and a receiver byte-count
plus BLAKE3 receipt. The recovery case imposes15% client-egress loss and50ms
delay for a1.503s window; it verifies2MiB rather than the4MiB ordinary transfer.
It proves completion under that controlled impairment, not standby validation,
physical-network recovery or user-visible desktop latency. One run and32 ping
samples cannot establish comparative superiority, reproducibility or broad
release acceptance. No milestone/checkpoint gate is closed by this increment.

The reproducible command family is
`rds-bench run --scenario CASE --backend BACKEND --iterations 32 --transfer-mib 4 --timeout-s 30`;
CASES are `ping`, `transfer` on both lanes and `recovery` on Noq. The versioned
[measurement method](../benchmark-transfer.md) remains authoritative. The full
corrected software CI passed both OS test lanes; the first Rust CodeQL attempt
ended during SARIF upload and its failed attempt remains retained. Final scanner
and installed acceptance are recorded independently in their deployment evidence.
