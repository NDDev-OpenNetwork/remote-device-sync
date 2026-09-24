# Versioned endpoint publisher — 2026-09-24

Scope: W1.5 signed mutations and publisher ownership, following record storage
`fdeabd9` and lock ownership `8a18064`. No deployment, wave closure or complete
directory acceptance claim. Fixtures contain synthetic identities only.

## Evidence

- `record_revisions`: ten tests cover same-second ordering in both stores,
  exact disk retry without another generation, conflicting equal revisions,
  deletion replay, exact validity boundaries, future/excessive lifetimes,
  domain and identity substitution, signature-before-decode, trailing bytes,
  collection/URL/wire bounds, durable counter restart and renewal behavior.
  Owned relay identities and IPv4/IPv6 locator validation are included.
- Publisher unit tests inject errors and abrupt child exits at five atomic
  write boundaries. Uncertain allocation returns no publishable bytes, poisons
  the live issuer and preserves complete old/new state for exact recovery.
- `publisher_lifecycle`: three real HTTP cases cover a committed publication
  whose success reply is lost, missing durable state before publication, and
  a server revision conflict reaching the supervisor without guessing a counter.
- The existing announce suite checks real publication/address changes and
  connection resolution. Its aged-record case now verifies both server refusal
  and independent client rejection of an expired record from a hostile server.
- Owned relay qualification now sends the actual `rds-relay://` advertisement
  through announce, directory storage and resolution before forcing the QUIC
  handshake, datagrams and stream exchange through the relay. A parser roundtrip
  also covers IPv6 brackets; this is not an IPv6 network qualification.

Review found that an HTTP-only URL restriction would break owned relay discovery.
The shared `OwnedRelayRoute` explicitly validates that separate scheme and public
identity. The old noq parser also failed to parse bracketed IPv6 hosts; the
shared typed socket parsing fixes this. Relay tests add two already-locked dev
dependencies for the real announce/directory path, with no new runtime library.

Two initial fixture failures were resolved without loosening validity:
`refresh_keeps_record_live` had manufactured future timestamps, and the resolver
expiry assertion only inspected the outer error string. Fixtures now use
explicit revisions and assert typed errors at both trust boundaries.

The first full workspace run passed the desktop latency smoke test but failed
policy reopen with `Busy`. The deterministic descriptor-alias reproducer and
the owner-lifetime correction are documented in the [lock receipt](rds-locks-20260924.md).
Original failed logs are retained separately from final runs.

## Final validation

Linux x86_64, Rust/Cargo 1.98.1:

- `cargo fmt --check`: passed.
- Default, X11 and all-feature workspace Clippy with warnings denied: passed.
- `cargo test --workspace`: **204 passed, 46 targets**, including the parallel
  desktop smoke test. No latency threshold was changed.
- `cargo test -p rds-net -p rds-agent -p rds-relay --all-features`:
  **58 passed, 18 targets**; this explicitly includes the owned relay tests.
- Agent binary build and `--help` with `--record-state`: passed.

An earlier feature run selected noq but omitted `rds-relay/owned-relay`; its
50 passing tests are not used as relay evidence. The final all-feature command
above replaces that incomplete feature selection. `cargo-deny` is not installed.
Private logs retain initial failures and each final command separately.

## Limits

The new record/delete wire and database format require a coordinated upgrade.
The migration utility is not implemented, so existing deployments remain on
their current format. Restoring all trusted state can roll back local checksums
and counters; an external GDS anchor remains required. Signed route lifetimes
are wall-clock checked; retained clock floors and expiry GC are still W1.5 work.
Memory/enrollment quotas, renewal fairness and strict HTTP framing remain open.

No native macOS, physical power loss/suspend, real GDS deployment, NAT matrix,
Cloudflare proxy or performance qualification was run. See the full
[record contract](../record-state.md). This change adds no runtime helper or
third-party dependency beyond the preceding Rust record-store library.
