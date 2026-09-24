# Endpoint configuration — 2026-09-25

Scope: first W2.1 implementation, with an owned relay bind correction. Base
`01935fc`. Linux x86_64, synthetic local fixtures only. No remediation wave closes.

## Confirmed defects

Two new transport tests failed against the base: iroh accepted extra local bind
addresses but used only the first, and noq accepted iroh relay URLs but never
used them. Both now return typed configuration errors before endpoint creation.

A third fixture, added while integrating the versioned configuration, failed
with `Address already in use` when an owned endpoint attached to a relay using
a fixed primary UDP port. The outer relay socket incorrectly tried to bind that
same port. It now uses a separate ephemeral port on the primary interface; the
real local relay fixture preserves the configured primary address and confirms
registration. Its initial test-source compile error was corrected before the
behavioral failure was recorded; only the subsequent behavioral run is evidence
for this defect.

## Implementation and coverage

`rds-net::EndpointSettings` is shared by `rds` and `rds-agent` through
`--endpoint-config`. The JSON schema requires version 1, rejects unknown fields
and incompatible backend/relay combinations, and bounds files, bindings, relay
origins and multipath settings. Endpoint identities and policy authorities are
outside this file. Explicit flags override a file, and repeated flags replace
lists without resetting unrelated settings. Owned relay locators reuse the
existing discovery parser and never become endpoint signing secrets.

The parser's own regression caught serde accepting extra fields on an internally
tagged unit variant. Empty struct variants enforce the unknown-field contract;
the test remains. All three shipped examples are synthetic and roundtrip in
supported builds; unavailable owned support is rejected in the isolated build.
The full workspace fixture also caught an overly strict duplicate-bind rule:
repeated port 0 requests are valid independent socket allocations. They remain
supported without changing the existing second-socket handshake test; repeated
fixed bindings are rejected.

Binary tests exercise both actual executables, requiring malformed flags/files,
oversized files, non-regular configuration and incompatible relay modes to fail
before an identity file is created. A positive CLI `id` test checks the persistent
key's public identity with a file and an explicit relay override, without network
startup. Lower-level bind tests require typed configuration errors, not incidental
socket failures. Existing owned relay handshake/datagram fixtures remain active.

The network crate now directly uses already-pinned `rustix` for bounded regular-file
loading and `thiserror` for typed configuration errors, and moves its existing
`serde_json` test dependency into runtime scope. No version was updated, no
additional package entered the lockfile, and no unsafe block or runtime helper
program was introduced.

## Final checks

Formatting, shared test-support formatting and default/X11/all-feature Clippy
passed with warnings denied. The final workspace run passed **285 tests across
53 targets**, with the already-qualified migration capacity case intentionally
ignored. The all-feature network/agent/CLI/relay run passed **72 tests across 24
targets**. Feature-isolated configuration and binary preflight checks passed
another **4** and **5** tests respectively.

[Machine-readable evidence](rds-endpoint-config-20260925-data.json) records exact
commands, build-job limits, durations, exit codes, counts and source hashes.
Full logs remain in private local evidence storage. `cargo-deny` is unavailable;
no unregistered checkpoint or wave closure was invoked.

The initial full build exhausted available disk space before tests started.
Only this workspace's generated incremental cache was removed after confirming
that its compilers had stopped, and subsequent builds used two compile jobs.
This infrastructure failure is not a runtime journal full-disk qualification.

## Remaining work


- Role-level service/authority configuration, uniform timeout classes, negotiation
  and session ownership remain W2.1–W2.8. Endpoint preflight does not yet validate
  every unrelated service flag before all side effects.
- This exposes the existing single-relay owned lane; it does not supply owned
  daemon deployment, multi-candidate/bootstrap racing, multi-relay failover or
  TCP/443 fallback. These remain W3/W4. Iroh stays the default.
- Native macOS, real NAT/egress/firewall changes and production rollout were not
  tested. Functional local tests do not establish latency percentiles or an
  IPv6/IPv4 reachability matrix.
- No live credentials, tenant/host inventory or raw external session content is
  present in examples or reports.
