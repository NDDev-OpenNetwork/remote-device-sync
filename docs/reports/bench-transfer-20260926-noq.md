# rds bench suite — 1790430061

commit: `1609b61`

## transfer-receiver-ack-v1 (noq, direct)

throughput: **3.7 MiB/s**

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 32993347
- `agent_rds_net_bytes_sent_total{via="direct"}` = 110341
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 5904
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 2390
- `agent_rds_net_live_paths` = 2
- `agent_rds_net_paths_seen_total{via="direct"}` = 2
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 2154
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 123564
- `client_rds_net_bytes_sent_total{via="direct"}` = 34333987
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 5808
- `client_rds_net_datagrams_sent_total{via="direct"}` = 23659
- `client_rds_net_live_paths` = 2
- `client_rds_net_paths_seen_total{via="direct"}` = 2
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_qnt_attempts_total` = 1
- `client_rds_net_qnt_success_total` = 1
- `client_rds_net_rtt_us` = 1026
- `client_rds_net_selected_path_known` = 1
- `transfer_completion_ns` = 8654234599
- `transfer_verified_bytes` = 33554432

- receiver-ack-v1: byte count + BLAKE3 digest + EOF; payload generation/hash, upload and receipt are timed; connect/OpenTcp excluded; not comparable to historical sender-finish results

