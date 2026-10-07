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
