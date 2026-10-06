# rds bench suite — 1791329947

commit: `5f2445d`

## migration (noq, relay-failover-drain)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 5 | 4.33ms | 5.91ms | 10.12ms | 10.12ms | 10.12ms | 6.99ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="relay"}` = 12476
- `agent_rds_net_bytes_sent_total{via="relay"}` = 28203
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 16294
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 109
- `agent_rds_net_live_paths` = 2
- `agent_rds_net_paths_seen_total{via="relay"}` = 2
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 1181
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="relay"}` = 5218
- `client_rds_net_bytes_sent_total{via="relay"}` = 14730
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 18000
- `client_rds_net_datagrams_sent_total{via="relay"}` = 43
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="relay"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 3004
- `client_rds_net_selected_path_known` = 1
- `migration_recovery_ns` = 19500375
- `migration_slot_after` = 1
- `phase_connect_ns` = 4906834
- `relay0_forwarded_bytes` = 14937
- `relay0_forwarded_datagrams` = 150
- `relay1_forwarded_bytes` = 15386
- `relay1_forwarded_datagrams` = 80

- relay-failover-drain: slot 0 failed → resumed 19.500375ms on slot 1, 0 probes lost in flight; post-migration RTT from the surviving slot

## migration (noq, relay-failover-kill)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 5 | 1.53ms | 1.61ms | 2.99ms | 2.99ms | 2.99ms | 1.96ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="relay"}` = 2822
- `agent_rds_net_bytes_sent_total{via="relay"}` = 3177
- `agent_rds_net_connections_accepted_total` = 2
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 5
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="relay"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="relay"}` = 4940
- `client_rds_net_bytes_sent_total{via="relay"}` = 14357
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 17719
- `client_rds_net_datagrams_sent_total{via="relay"}` = 32
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="relay"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 1298
- `client_rds_net_selected_path_known` = 1
- `migration_recovery_ns` = 113465041
- `migration_slot_before` = 1
- `phase_connect_ns` = 7009959
- `relay0_forwarded_bytes` = 16184
- `relay0_forwarded_datagrams` = 63
- `relay1_forwarded_bytes` = 11242
- `relay1_forwarded_datagrams` = 46

- relay-failover-kill: slot 1 failed → resumed 113.465041ms on slot 0, 0 probes lost in flight; post-migration RTT from the surviving slot

