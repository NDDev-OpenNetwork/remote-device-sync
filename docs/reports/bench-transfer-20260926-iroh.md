# rds bench suite — 1790430042

commit: `1609b61`

## transfer-receiver-ack-v1 (iroh, direct)

throughput: **4.4 MiB/s**

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 31709635
- `agent_rds_net_bytes_sent_total{via="direct"}` = 133991
- `agent_rds_net_congestion_events_total` = 1
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 5808
- `agent_rds_net_datagrams_lost_total{via="direct"}` = 3
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 1521
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_rtt_us` = 3517
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 139020
- `client_rds_net_bytes_sent_total{via="direct"}` = 34372548
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 131210
- `client_rds_net_datagrams_sent_total{via="direct"}` = 23777
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_rtt_us` = 3852
- `client_rds_net_selected_path_known` = 1
- `transfer_completion_ns` = 7355291503
- `transfer_verified_bytes` = 33554432

- receiver-ack-v1: byte count + BLAKE3 digest + EOF; payload generation/hash, upload and receipt are timed; connect/OpenTcp excluded; not comparable to historical sender-finish results

