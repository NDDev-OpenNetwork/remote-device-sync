# Destination-bound grants and renewable connection leases

Status: **W2.3 partial**, Linux implementation and regression evidence in the
[dated receipt](reports/rds-grant-leases-20260925.md). This is a breaking signed
token/local IPC update. It does not implement an authority service or close W2.

## Signed contract and issuer responsibilities

`rds-core::grant::GrantPayload` version **2** contains `version`, positive
`revision`, issuer, subject, audience, a 128-bit nonce, services, validity times
and constraints. Subject must match the QUIC-authenticated peer; audience must
match the serving agent's actual endpoint key, never a request-supplied value.
Membership in the agent allowlist and trusted issuer set remains required.

Ed25519 signs `rds/capability-grant/v2\0` followed by the postcard payload.
Strict verification rejects raw-payload signatures, wrong versions, trailing
bytes, signatures of the wrong size, payloads exceeding 4 KiB and excessive
collections. Expiry must be strictly greater than `not_before`. The existing
30-second future `not_before` tolerance does not relax expiry or the configured
TTL cap. Clock arithmetic saturates instead of overflowing.

The stable revocation/replay ID is BLAKE3 over `rds/grant-session/v2\0`, issuer,
subject, audience and nonce, in that order with fixed field widths. The issuer
must generate a fresh cryptographically random nonce for every new authorization
session; it retains that nonce only for renewal of the same session. `Grant::id`
parses this identity but **does not verify the signature**. Revocation producers
must derive IDs from issued/verified grants. The session revision is not a
registry epoch, tenant identifier or estate policy revision.

Use the existing ed25519-dalek and BLAKE3 libraries. No new cryptography library,
external executable, key store, dependency or unsafe block is introduced.

## Directional service permissions

The 2026-09-26 increment adds fine capabilities to the signed `services` list:

| Capability | Agent-side permission |
|---|---|
| `SyncRead` | Download from the configured sync root (`Request`). |
| `SyncWrite` | Upload to the configured sync root (`Offer`). |
| `DesktopView` | Open a desktop session, receive frames, steer the encoder and exchange heartbeats. No input injection. |
| `DesktopControl` | Input modifier; requires `DesktopView` to open a session. |
| Legacy `Sync` | Both read and write, preserving its original meaning. |
| Legacy `Desktop` | Both view and input, preserving its original meaning. |

Prefer narrow capabilities when issuing new grants. Permissions are additive:
adding `SyncRead` to a legacy `Sync` grant does **not** remove write permission.
`DesktopControl` alone grants neither viewing nor input. Existing display/port
constraints still apply. These are whole-root sync permissions, not per-path
ACLs; account, tenant and policy-revision binding remain open.

The sync handler checks direction immediately after decoding the first bounded
message, before path validation, metadata reads, manifests, journals or writes.
A denial emits a fixed `SyncMsg::Refuse` without filesystem details. A scoped
connection remains usable. Its exclusive sync slot is RAII-owned across the
hello ACK and body, including failed writes and cancellation.

View-only sessions never create an input worker or invoke a sink. Controlling
sessions lazily create one bounded worker, reuse its backend and run blocking
native calls outside Tokio's async executor. The existing session-display check
precedes dispatch. `InputAck` is emitted only after the sink returns success;
denied, unavailable or failed input gets no ACK. The current wire has no typed
input-rejection event; a heartbeat response is not an input acknowledgment.
An ACK means backend acceptance, not proof that a target application reacted.
Cancellation discards queued input; a native call already in progress cannot
be undone. Physical desktop/input and stuck-syscall qualification remain open.
The wire display-ID check is not a native seat/focus isolation boundary. X11
screen selection, focus, key/button mapping and held-key release remain W6.4;
the synthetic sink tests do not qualify those behaviors.

New `ServiceKind` tags are appended as 6–9; tags 0–5, grant payload version 2,
local IPC version 3, `StreamHello` and service framing are unchanged. Agents
continue advertising coarse services in `AgentInfo`. Old decoders reject a
grant containing an unknown fine capability; no automatic fallback to a broad
grant is permitted. Upgrade agents before issuing these scopes. General
capability negotiation remains W2.2. Renewal still requires the exact same
service list: adding, removing or reordering capabilities needs a new session.

## Renewal and cancellation

`StreamHello::RenewAuthz` operates on the already authenticated connection.
The issuer signs a new payload with the same identity, exact service list and
constraints, a higher revision, a nondecreasing `not_before` and a strictly
later expiry. It must pass the same signature/audience/time/TTL checks as initial
admission. Both the current and replacement grants must remain live at commit.
Renewal cannot revive an expired/closed connection. Exact duplicate payloads
are idempotent and retain the original accepted deadline.

One connection owns one replay reservation and one watchdog across all
revisions. Revoking the stable ID closes the connection even after renewal.
The watchdog continues to enforce the old lease during renewal; a stalled reply
does not freeze expiry. Scope changes, including reductions, are refused by
renewal: revoke the old ID and authorize a new session with a fresh nonce.
In-place service-specific scope reductions remain future work.

Each accepted lease combines strict wall-clock expiry with the existing
suspend-aware continuous clock/boot-identity adapter. Wall rollback before the
acceptance anchor, boot changes and unavailable clocks fail closed. A duplicate
does not obtain a fresh continuous-clock budget. The watchdog samples at most
once per second and wakes on renewal/revocation/connection closure; actual
closure includes scheduler delay. Service admission checks validity directly.
Native suspend/macOS behavior still needs physical platform qualification.

The server writes OK, commits authorization, then finishes the response stream.
The client requires both OK and EOF under one bounded authorization exchange.
Failed/canceled verification, reply or commit after transaction acquisition
closes that connection and releases ownership. An uncertain client exchange
also closes it. No SSH command, transfer or TCP request is replayed.

The existing per-connection task limit includes control work. Grant mode
requires at least two slots and reserves one from long-lived service bodies:
with the default 64 total slots, at most 63 such bodies coexist. A full service
budget refuses another service without closing the connection; renewal still
has worker capacity. Incomplete/malicious preludes can occupy control capacity
until their 15-second deadline. This is not an anti-DoS scheduling guarantee.
Allowlist-only mode retains its existing task capacity.

## Managed use and upgrade

Obtain a signed replacement from the trusted issuer, then target an explicit
session ID (changing the selected device cannot redirect renewal):

```sh
rds session renew --session <session-id> --grant-file /private/renewed-grant.json
```

The manager keeps the same endpoint, connection and service streams. It stores
only a credential digest, serializes renewals/connect reuse and updates that
digest after remote success. Reconnect/reuse must then supply the current
grant; the old digest produces `CredentialConflict`. Local cancellation or
failed replies during the reserved transaction remove that pinned session.
Non-streaming local calls require reply plus EOF, which follows publication of
reserved state; extra reply bytes are rejected. A lost response after successful
publication remains an uncertain RPC outcome, inspectable through `session list`.
Neither another device nor a remote command is selected/replayed automatically.

Local IPC is **version 3**. Upgrade CLI and local agent together; incompatible
versions fail before session mutation. Remote ALPN `rds/0` and existing service
framing are unchanged. The signed grant is independently versioned and the new
renewal variant is appended; old servers reject unsupported renewal without a
downgrade/reconnect fallback. General capability negotiation remains W2.2.

Old grant files/signatures and old payload-hash revocation IDs are incompatible.
Upgrade issuer, agent and client as one coordinated change; revoke/drain old
sessions with the old policy before cutover, then issue fresh v2 grants and
stable-ID revocation snapshots. Do not reinterpret old grants, reuse their
nonces or fall back to unsigned/allowlist-only access. This repository change
does not perform the installed-binary/authority migration.

There is no automatic issuer API, credential watcher or renewal scheduler yet.
Operators/library callers must provide the next signed revision before expiry,
with time for request deadlines and retries. The authority API and automatic
end-to-end renewal remain W4.2, rather than silently extending authorization.

## Observations and remaining acceptance

Fixed `grant_authorize` and `grant_renew` operation names export outcome and
elapsed time through the bounded Rust telemetry and Vector projection. Grant
payloads, signatures, endpoint identifiers, nonces and revision values are not
added to the allowlisted JSON schema. Private text diagnostics retain their
existing private-data rules. Upgrade the Vector projection with the binaries;
older operation allowlists drop these new records. No live collector/sink or
alert route is changed.

Required next: tenant/policy binding, per-path and account scopes;
automatic GDS issuance/renewal and policy reconciliation; viewer/sync manager
integration; native macOS and suspend tests; mixed SSH/video/sync, physical
WAN/NAT/relay and long-running resource/latency acceptance. Loopback tests and
injected clocks do not establish those release guarantees.
