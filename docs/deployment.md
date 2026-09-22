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
| 3341 | tcp | discovery HTTP API (`PUT/GET/DELETE /v1/records`, `/v1/registry`, `/v1/revocations`, `/v1/health`) | public to estate members; writes are signature-verified |
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
