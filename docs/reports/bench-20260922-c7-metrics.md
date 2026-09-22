# rds bench suite — 1790074558

commit: `1a8f649`

## ping (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 10 | 1.07ms | 1.39ms | 1.84ms | 1.84ms | 1.84ms | 1.47ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 9239
- `agent_rds_net_bytes_sent_total{via="direct"}` = 8492
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 13200
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 19
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_rtt_us` = 12669
- `client_rds_net_bytes_received_total{via="direct"}` = 8277
- `client_rds_net_bytes_sent_total{via="direct"}` = 9468
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 12000
- `client_rds_net_datagrams_sent_total{via="direct"}` = 42
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_rtt_us` = 988


