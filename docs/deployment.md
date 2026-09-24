# Deployment — rds-server / rds-agent

Reference deployment: `rds-server` (relay + discovery directory) and
`rds-agent` on a Linux host under systemd. Units live in
`deploy/systemd/`; copy them to `/etc/systemd/system/`.

## Install

```sh
# binaries (release build)
cargo build --release -p rds-server -p rds-agent -p rds-cli
install -m0755 target/release/rds-server /usr/local/bin/
install -m0755 target/release/rds-agent  /usr/local/bin/
install -m0755 target/release/rds-cli    /usr/local/bin/

# users + state
useradd --system --home /var/lib/rds       --shell /usr/sbin/nologin rds
useradd --system --home /var/lib/rds-agent --shell /usr/sbin/nologin rds-agent
install -d -m0700 -o rds       -g rds       /var/lib/rds/directory
install -d -m0700 -o rds-agent -g rds-agent /var/lib/rds-agent

install -m0644 deploy/systemd/rds-server.service /etc/systemd/system/
install -m0644 deploy/systemd/rds-agent.service  /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now rds-server rds-agent
```

The agent creates `endpoint.key` on first start (`0600`, enforced by
`load_or_create_key`). The endpoint id it publishes is derived from
that key — keep it across reinstalls or the device's identity changes.
`rds id` prints the bare hex id needed for `--allow` lists.

**One key, one role.** Never let an agent and an operator CLI share a
key file: two live endpoints with the same EndpointId make the relay
disconnect the earlier session ("Another endpoint connected"). Give
operator CLIs their own key (`rds --key-file ~/.config/remote-device-sync/cli.key …`)
and list their ids in the agent's `--allow` too. The relay's `--allow`
covers every endpoint that may *use* the relay — servers and clients
alike.

## Ports and firewall

| Port | Proto | Service | Exposure |
| --- | --- | --- | --- |
| 3340 | tcp | relay (iroh relay protocol over HTTP) | public — endpoints behind NAT dial in |
| 3341 | tcp | discovery HTTP(S) API (`PUT/GET/DELETE /v1/records`, `/v1/registry`, `/v1/revocations`, `/v1/health`) | estate members; configure directory TLS for lookup confidentiality; writes are signature-verified |
| 3341 | tcp | `GET /v1/metrics` | **loopback only** — the router returns 404 to non-loopback peers; scrape over SSH (`ssh -L`) or a local exporter |
| — | udp | endpoint QUIC data path + hole-punching | endpoints need outbound UDP; inbound UDP only required to *receive* direct paths (relay fallback covers its absence) |

Minimal nftables for the services host:

```nft
table inet rds {
    chain input {
        type filter hook input priority 0; policy drop;
        ct state established,related accept
        iif lo accept
        tcp dport { 22, 3340, 3341 } accept
    }
}
```

`rds-agent` needs no inbound allow rule: it dials the relay outbound and
accepts direct paths opportunistically (hole-punched or via the
endpoint's discovered addresses).

**Relay redundancy.** `--relay` is repeatable on both `rds` and
`rds-agent`; iroh probes all configured relays, homes on the
lowest-latency one, and fails over automatically. Production should run
≥2 `rds-relay`/`rds-server` instances in different failure domains and
list all their URLs — no LB or failover plumbing is needed on the relay
side, since every endpoint carries the full list.

**TLS on the relay.** `rds-server` serves the iroh relay protocol
(WebSocket over HTTP) on 3340 in plaintext: relayed payloads are
end-to-end-encrypted QUIC the relay cannot read, and relay admission is
keyed by `EndpointId` signature challenge, so a network MITM can only
disrupt, not decrypt. For defence-in-depth on a public IP the relay
supports native TLS in two modes:

```bash
# manual PEM (certbot, internal CA, any provider)
rds-server --tls-cert /etc/rds/cert.pem --tls-key /etc/rds/key.pem

# or in-process Let's Encrypt (TLS-ALPN-01 — requires port 443
# reachable from the internet, so a privileged bind or a redirect)
rds-server --tls-acme-domain relay.example.com \
           --tls-acme-contact mailto:ops@example.com \
           --tls-acme-cache /var/lib/rds/acme
```

HTTPS binds `--tls-https-addr` (default `0.0.0.0:3443`; the
unprivileged unit cannot bind 443 — for 443 add
`AmbientCapabilities=CAP_NET_BIND_SERVICE` to the unit). The plaintext
HTTP port keeps serving only the captive-portal probe. Endpoints then
dial `--relay https://<host>:<port>`; the relay protocol rides WebSocket
inside TLS unchanged. A TCP-level TLS terminator (nginx `stream`,
haproxy) in front of 3340 remains a valid alternative.

The relay serves `GET /healthz` → `200` on its HTTP(S) listener for
load-balancer and monitoring probes.

**Tunnel/VPN integration is optional.** RDS does not invoke tunnel or VPN
helper processes. Direct connections use UDP; the relay and directory have
separately configured TCP listeners. Cloudflare Tunnel/WARP may be evaluated
as estate deployment options under W3, using measured setup latency, ongoing
SSH/desktop/sync traffic, idle timeout, reconnect and outage behavior. The
native directory HTTPS tests below do not qualify a Cloudflare route or any
other edge proxy. Keep endpoint identity and policy independent of whichever
route is selected, and retain a separately tested recovery path.

### Native directory HTTPS

The directory supports its own TLS listener inside `rds-server`, with no
proxy/helper process required. It is separate from relay TLS: supplying
`--tls-cert` alone does not encrypt the directory. Example configuration
(synthetic hostname and paths):

```sh
rds-server --http-addr 0.0.0.0:3341 \
  --directory-tls-cert /etc/rds/directory-chain.pem \
  --directory-tls-key /etc/rds/directory-key.pem
rds --server https://directory.example.com:3341 \
  --registry-key <base32-verifying-key> ping device-a
rds-agent --directory https://directory.example.com:3341 <agent-policy-flags>
```

The certificate must cover the configured hostname (or IP SAN for an IP URL).
Clients use public WebPKI roots by default. For a private CA, supply
`--directory-ca /etc/rds/directory-ca.pem` to `rds` and `rds-agent`; the bundle
**replaces** public roots. Empty/invalid bundles and a CA flag on an HTTP
endpoint are errors. There is no skip-verification option, redirect following
or HTTP fallback. The registry verifying key remains separately provisioned;
a TLS certificate cannot authorize a name binding.

Client origins accept `https://host[:port]`, explicit `http://host[:port]`,
or legacy `IP:port` (plaintext). Paths, credentials, queries and fragments are
refused. DNS is resolved on each request with a bounded, staggered dial race;
all work shares a default 3-second deadline. The directory uses HTTP/1.1 with
Content-Length framing. Compatibility with particular edge proxies/tunnels
has not been qualified by these loopback TLS tests.

When directory TLS is enabled, `--http-addr` accepts only TLS. Health probes
must also use HTTPS with normal certificate validation. The listener uses
the same connection cap and a 10-second absolute deadline including TLS.
Provision a renewed certificate and restart the server to load it; directory
hot reload/ACME is not implemented. Keep the PEM key readable only by the
service identity. A loopback reverse proxy changes the source address seen by
the metrics route; if one is introduced, keep `/v1/metrics` private at that
boundary instead of relying on the directory's peer-IP check.

## Sandboxing

Both units set `NoNewPrivileges`, `ProtectSystem=strict`,
`ProtectHome`, `PrivateTmp`, `PrivateDevices`, `ProtectKernel*`,
`ProtectControlGroups`, `RestrictSUIDSGID`, `LockPersonality`,
`RestrictRealtime`, `CapabilityBoundingSet=` (empty),
`RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`,
`SystemCallFilter=@system-service`. `rds-server` additionally sets
`MemoryDenyWriteExecute`. Writable paths are limited to the unit's
`StateDirectory` via `ReadWritePaths`.

Caveats:

- **Agent desktop service**: X11 capture and input injection need a
  session display and `/dev/uinput` — run the agent as a user service
  in the graphical session for desktop, or relax `PrivateDevices` and
  add `DeviceAllow=/dev/uinput rw` plus `SupplementaryGroups=input`.
  `PrivateTmp` also hides `/tmp/.X11-unix` from the unit — the session
  X socket won't be reachable unless the display listens on the
  abstract socket (default on Linux) or `PrivateTmp` is dropped.
- **Agent `--sync-dir`**: add its path to `ReadWritePaths` or keep it
  under `/var/lib/rds-agent`.
- **Key material**: `endpoint.key` must stay `0600` and owned by the
  service user (`stat -c '%a %U' /var/lib/rds-agent/endpoint.key`
  → `600 rds-agent`). The estate signing key never lives on this host —
  `--registry-key`/`--revocations-key` are verifying (public) keys.

## Runbook

### Signed name lookup migration

The name API returns a per-name signature proof. Upgrade the registry issuer,
directory and name-using clients together: re-create snapshots with the updated
`SignedRegistry::sign` or `publish` so they contain `bindings`, and distribute
the registry **public** key independently through GDS/configuration. The issuer
private key stays with the estate. Names use lowercase ASCII letters, digits
and hyphens (1–63 bytes); snapshots have at most 256 names and must fit the
256 KiB HTTP body limit. Each binding is valid for at most 24 hours; issuer and
clients need synchronized clocks (`issued_at <= now < expires_at`).

```sh
rds --server 127.0.0.1:3341 --registry-key <base32-verifying-key> ping device-a
```

New clients refuse missing anchors and unsigned legacy responses. Existing
tickets and pinned endpoint-key lookups retain their independent trust path.
The directory refuses expired names even if no newer snapshot has arrived.
The CLI uses a durable 1024-name freshness cache and a global registry revision;
it refuses capacity overflow instead of evicting anti-rollback history.
Snapshots now require positive authority epoch/revision metadata, registry/name
signature domain v2, and revocation domain v1. The issuer must persist revision
allocation and publish renewals. See [durable policy migration](policy-state.md)
for state paths, dual-signed authority rotation and reboot behavior. Managed
`--issuer` mode also requires `--directory` and `--revocations-key`. A missing or
stale revocation lease closes admission and live connections; a fetch failure
cannot replace the denylist or extend its lifetime. HTTPS protects the lookup exchange;
signatures preserve identity verification independently of that transport.

### Restart

```sh
systemctl restart rds-server        # ~1s outage; clients reconnect via backoff
journalctl -u rds-server -f         # watch "relay listening" / "directory listening"
curl -sf http://127.0.0.1:3341/v1/health   # {"ok":true}
```

### Upgrade

```sh
install -m0755 target/release/rds-server /usr/local/bin/rds-server.new
mv /usr/local/bin/rds-server.new /usr/local/bin/rds-server
systemctl restart rds-server
journalctl -u rds-server --since -1m     # confirm clean start
```

Records in `/var/lib/rds/directory` persist; clients re-announce within
their TTL window (default 300 s), so a restarted directory refills
itself. To roll back, reinstall the previous binary and restart.

### Drain / decommission

```sh
systemctl stop rds-server    # relay closes; endpoints fall back to
                             # direct paths or the next configured relay
```

There is no in-service drain flag yet: stopping the relay drops
relayed connections, which retry through their remaining relays or
migrate to direct paths. Announce the drain in advance for attended
sessions.

### Failure modes

The W1.5 record-store format is changing. Legacy per-key JSON directories now
refuse startup instead of discarding replay history. Deployment migration is
pending the versioned publisher work; see [record-state.md](record-state.md).
Preserve the complete directory, and never remove an anchor to force startup.

| Symptom | Likely cause | Check | Fix |
| --- | --- | --- | --- |
| `rds ssh`/`ping` never connects | relay unreachable | `curl http://<host>:3340/` ; `ss -tlnp \| grep 3340` | open tcp/3340, restart unit |
| connects but only via relay, no direct upgrade | UDP blocked both ways | metrics: `paths_seen{via="direct"}` stays 0 | allow outbound UDP; check NAT hairpin |
| `GET /v1/records/<id>` 404 after restart | records expired pre-restart | directory files under `/var/lib/rds/directory` | clients re-announce within TTL; lower `--record-ttl` |
| `PUT /v1/registry` refused | missing/wrong `--registry-key` | unit `ExecStart` args | install the estate verifying key |
| service refuses grant-mode streams | denylist stale or clock skew | `journalctl -u rds-agent`, `timedatectl` | fix clock; confirm `--revocations-key` matches estate |
| `/v1/metrics` 404 from remote host | by design | scrape via `ssh -L 3341:127.0.0.1:3341` | — |
| unit fails `ProtectSystem` writes | state dir mis-ownership | `journalctl -u …` shows EROFS/EACCES | `chown -R rds:rds /var/lib/rds` |
| relay floods under open access | `--allow` unset on public IP | connection count in logs/metrics | set `--allow` to estate EndpointIds |

### Observability quick reference

```sh
# directory counters (loopback only)
curl -s http://127.0.0.1:3341/v1/metrics
# per-session structured log
journalctl -u rds-agent -o json | jq 'select(.fields.session_id == "…")'
```
