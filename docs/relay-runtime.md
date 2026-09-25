# Relay binary configuration and lifecycle

W3.2 integration uses the same `rds-relay::RelayArgs` and runtime for
`rds-relay` and `rds-server`. Iroh remains the default/comparison until the
owned transport meets the network, operational and performance gates. This
change adds no external daemon, command wrapper or new dependency version.
Native crypto/OS dependencies still exist; this is not a pure-Rust transitive
dependency claim.

## Backend selection

| Setting | Iroh (default) | Owned QUIC (`noq`) |
|---|---|---|
| Build | Default features | `rds-relay/owned-relay` or `rds-server/owned-relay` |
| Bind | HTTP TCP; optional separate HTTPS | UDP QUIC |
| Address flag | `--addr` in relay; `--relay-addr` in server | Same flags, interpreted as UDP |
| Identity | Existing iroh relay behavior | Required `--relay-key-file`, separate from device/authority keys |
| Admission | Existing `--allow`; empty retains legacy open mode | Nonempty `--allow` required; unknown keys denied |
| Development | Existing iroh behavior | Explicit `--development-open-relay`, incompatible with `--allow` |
| Connection cap | Existing iroh implementation | `--relay-max-connections`, positive u16, default 256 |
| TLS flags | Manual PEM or in-process ACME | HTTP TLS/ACME options rejected; QUIC already authenticates its pinned key |

The owned cap includes in-progress handshakes and registrations. Releasing a
connection returns capacity. It is not a global CPU/RSS/FD qualification. Relay
admission, directory enrollment and agent service authorization remain distinct
checks: admitting an identity to forward packets does not authorize a session.

```sh
cargo build --locked -p rds-server --features owned-relay
cargo build --locked -p rds-relay --features owned-relay
cargo build --locked -p rds-agent -p rds-cli \
    --features rds-agent/transport-noq,rds-cli/transport-noq

# Replace placeholders with separately provisioned identities and owned state.
rds-server --relay-backend noq --relay-addr 0.0.0.0:3340 \
    --relay-key-file /var/lib/rds/relay/identity.key \
    --relay-max-connections 256 --allow <operator-id> --allow <device-id> \
    --http-addr 127.0.0.1:3341 --directory /var/lib/rds/directory \
    --directory-allow <base32-device-key>

# Standalone relay uses --addr instead of --relay-addr.
rds-relay --relay-backend noq --addr 0.0.0.0:3340 \
    --relay-key-file /var/lib/rds/relay/identity.key \
    --allow <operator-id> --allow <device-id>

# Use the relay endpoint ID printed at readiness and its reachable socket.
rds-agent --backend noq --owned-relay rds-relay://<relay-id>@<ip>:3340 \
    --allow <operator-id> --ssh 127.0.0.1:22
rds --backend noq --owned-relay rds-relay://<relay-id>@<ip>:3340 ping <device-ticket>
```

The two relay commands above are alternatives, not concurrent owners of the
same role/socket. The example still uses the current TCP bridge to SSH;
native Rust SSH/PTY is separate W5 work. Private deployment paths and actual
identity pins belong in the estate. Do not reuse a device key for the relay.
The directory example binds loopback; remote directory exposure needs its own
HTTPS configuration and the existing policy/enrollment contracts.

## Preflight and persistent state

Clap validates typed flags. `RelayArgs::prepare` checks backend compatibility
and reads/parses manual PEM once. Chain input is capped at 1 MiB, private key
input at 64 KiB. Invalid/mismatched PEM, unused HTTPS bind flags, incomplete
ACME options, owned flags in iroh mode and HTTP TLS flags in owned mode fail
before identity/catalog creation or listeners. The `noq` value remains
recognizable without its feature and produces an explicit backend-unavailable
error. Existing iroh flags and the public `serve` signature remain available;
previously ignored irrelevant flags now fail.

The GDS host also parses enrollment, authority key/epoch, bounded registry JSON,
bounded rotation inputs and directory TLS before initialization. Explicit epoch
zero or epoch/registry options without a bootstrap key fail. A rotated registry
is validated against durable current policy when that policy is opened, not
incorrectly against an old bootstrap key during read-only preflight.

`PreparedRelay::initialize` loads/creates the owned identity on a blocking
worker, using the [identity transaction contract](identity-storage.md).
`ReadyRelay::bind` starts sockets only afterward. The server then opens its
catalog/policy and starts directory and relay. Failure to bind the relay awaits
directory shutdown. A valid new key survives later I/O/startup failure; startup
is not an atomic transaction across every service/store. Existing malformed
keys are preserved and rejected, never regenerated in place.

## Readiness, shutdown and verification limits

Final relay readiness follows successful bind and Unix SIGINT/SIGTERM handler
installation. Owned mode prints its actual UDP socket and public endpoint ID.
This establishes local listener readiness, not external reachability, NAT
qualification or automatic monitoring/restart of every background task.

Both binaries await relay shutdown; the composed host joins directory shutdown
as well. Owned shutdown uses the [checked drain lifecycle](relay-control.md#server-task-ownership)
and propagates retained runner errors to process failure. The existing drain
notice/grace protocol is unchanged. Drop cannot synchronously join and durable
filesystem work has no forced kernel-I/O deadline. Startup cancellation,
runtime failure supervision and service-wide timeout policy remain open.

Real-binary fixtures cover flag/PEM preflight, feature-off rejection, iroh and
manual-TLS startup, unknown-peer refusal, explicit development admission,
connection-cap recovery, inner authenticated stream/datagram exchange, signed
directory publication/resolution, persistent relay identity/catalog reuse and
SIGTERM drain. Fixtures pin inner connections to one relay path. Their clients
use the Rust endpoint library, not a qualified full SSH/desktop/sync session.
The tests run in both server packages; CI includes owned-server tests on both
supported OSes. Local Linux results do not establish native macOS acceptance,
internet reachability, UDP-blocked fallback, warm replacement, throughput or
latency targets. All remediation waves remain open.
