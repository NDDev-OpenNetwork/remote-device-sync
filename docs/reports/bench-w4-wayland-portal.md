# rds bench suite — 1791687711

commit: `90b361cf`

## handshake (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 0.40ms | 1.16ms | 1.34ms | 1.50ms | 4.50ms | 0.98ms |

attempts: 100/100 succeeded

metrics:
- `agent_rds_net_connections_accepted_total` = 99
- `client_rds_net_bytes_received_total{via="direct"}` = 136456
- `client_rds_net_bytes_sent_total{via="direct"}` = 496825
- `client_rds_net_connections_opened_total` = 100
- `client_rds_net_datagrams_sent_total{via="direct"}` = 597
- `client_rds_net_paths_seen_total{via="direct"}` = 100


