# rds bench suite — 1791680564

commit: `7f5f385`

## handshake (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 0.36ms | 0.76ms | 1.19ms | 1.29ms | 1.43ms | 0.77ms |

attempts: 100/100 succeeded

metrics:
- `agent_rds_net_connections_accepted_total` = 99
- `client_rds_net_bytes_received_total{via="direct"}` = 133800
- `client_rds_net_bytes_sent_total{via="direct"}` = 495217
- `client_rds_net_connections_opened_total` = 100
- `client_rds_net_datagrams_sent_total{via="direct"}` = 590
- `client_rds_net_paths_seen_total{via="direct"}` = 100


