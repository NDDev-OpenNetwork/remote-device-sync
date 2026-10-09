# Noq source CodeQL review — 2026-10-09

Review baseline: `93be1f014329e2ea7d573c209c514a67d4b22fb8`. A successful scanner
workflow left 30 open alerts in vendored Noq. Each exact source location and
reported sink was reviewed; no query, scanner feature or directory exclusion
was changed. This is a scoped disposition, not a general security certification.

| Alerts | Source | Finding and disposition |
|---|---|---|
| 73–93 | `vendor/noq-proto/src/congestion.rs`, `Controller` default hooks | Deliberately empty optional callbacks explicitly allow unused parameters. Concrete controllers override relevant callbacks. Named arguments document the published interface; accepted as intentional upstream behavior (`won't fix`), not mislabeled as repaired code. |
| 94–99 | `vendor/noq-proto/src/connection/qlog.rs`, `AddAddress`, `ObservedAddr`, `ReachOut` conversions | Opposite-family match arms return `None`; matching-family arms serialize the address. The observation module permits unused variables. No routing/security work is omitted. Accepted intentional upstream quality findings (`won't fix`). |
| 100 | `vendor/noq-proto/src/varint.rs:169` → `token.rs:176` | The decoder's initialized buffer is populated from input before the width-specific conversion. The reported sink is the token replay log. `Token::new` samples a 128-bit nonce from `CryptoRng`; `Token::decode` reads all 16 nonce bytes and authenticates with `key.open` before use. This is not constant nonce generation (`false positive`). |
| 101 | `crypto/rustls.rs`, `RETRY_INTEGRITY_KEY_DRAFT` | Public interoperability constant mandated by [QUIC TLS draft-32 §5.8](https://www.ietf.org/archive/id/draft-ietf-quic-tls-32.txt), used for Retry integrity tags. Not a deployment secret or TLS traffic key (`false positive`). |
| 102 | `crypto/rustls.rs`, `RETRY_INTEGRITY_KEY_V1` | Public interoperability constant mandated by [RFC 9001 §5.8](https://www.rfc-editor.org/rfc/rfc9001.txt). Randomizing it would break Retry verification; it is separate from encrypted token key generation (`false positive`). |

The three cryptographic alerts do not justify replacing QUIC cryptography or
weakening checks. The other 27 are documented intentional unused bindings, not
an exception for future unused variables in RDS code. Provider review comments
bind these exact alerts to the baseline; future instances still require review.
