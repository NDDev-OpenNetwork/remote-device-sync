# rds bench suite — 1791707342

commit: `9c56c952`

## handshake (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 0.34ms | 0.44ms | 1.20ms | 1.40ms | 3.21ms | 0.62ms |

attempts: 100/100 succeeded

metrics:
- `agent_rds_net_bytes_received_total{via="direct"}` = 16410
- `agent_rds_net_bytes_sent_total{via="direct"}` = 17066
- `agent_rds_net_connections_accepted_total` = 99
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 23
- `agent_rds_net_paths_seen_total{via="direct"}` = 2
- `client_rds_net_bytes_received_total{via="direct"}` = 178507
- `client_rds_net_bytes_sent_total{via="direct"}` = 521415
- `client_rds_net_connections_opened_total` = 100
- `client_rds_net_datagrams_sent_total{via="direct"}` = 638
- `client_rds_net_paths_seen_total{via="direct"}` = 100


