# Directory reconcile model — 2026-10-08

This wave adds the planning layer after the scanner and snapshot wire model.
It is deliberately filesystem-free: no service route, journal write or
capability advertisement changes.

The opaque token and conditional-update shape follows the relevant safety
principles from [RFC 6578](https://www.rfc-editor.org/rfc/rfc6578.html)
(opaque collection synchronization tokens) and [RFC 4918](https://www.rfc-editor.org/rfc/rfc4918.html)
(conditional writes and lock-aware overwrite prevention), without claiming
WebDAV wire compatibility.

The model provides:

- opaque manifest revisions and an optimistic destination revision
  precondition, so an apply cannot silently overwrite a destination that
  changed after planning;
- deterministic one-way operations (`Put`, `Replace`, exact-identity `Move`)
  with explicit `Keep` or `Delete` policy;
- tombstones carrying the previous entry and source revision, which apply code
  must journal before deletion and retain until convergence;
- bounded canonical operation ordering and a stable BLAKE3 plan digest;
- three-way conflict preview that reports paths changed differently on both
  sides and never picks an automatic winner.

The next gate must bind this plan to held destination handles, the existing
receive locks and crash-safe journal commit boundaries. Recursive service
admission, watch/reconcile, metadata policy and two-way apply remain open.

Validation:

```text
cargo fmt --check
cargo clippy -p rds-sync --all-targets -- -D warnings
cargo test -p rds-sync directory::reconcile
```
