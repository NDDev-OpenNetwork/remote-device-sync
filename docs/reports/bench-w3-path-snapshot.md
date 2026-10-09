# rds bench suite — 1791584433

commit: `971de4b`

## handshake (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 0.58ms | 0.90ms | 1.16ms | 1.82ms | 5.45ms | 0.97ms |

attempts: 100/100 succeeded

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 3748
- `agent_rds_net_bytes_sent_total{via="direct"}` = 4136
- `agent_rds_net_connections_accepted_total` = 100
- `agent_rds_net_cwnd_bytes` = 12145
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 6
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_rtt_us` = 314
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 136598
- `client_rds_net_bytes_sent_total{via="direct"}` = 496690
- `client_rds_net_connections_opened_total` = 100
- `client_rds_net_datagrams_sent_total{via="direct"}` = 594
- `client_rds_net_paths_seen_total{via="direct"}` = 100


