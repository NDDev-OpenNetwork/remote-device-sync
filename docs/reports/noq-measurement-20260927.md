# rds bench suite — 1790468071

commit: `67d73fc`

## handshake (noq, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 21.73ms | 24.87ms | 41.00ms | 43.30ms | 43.30ms | 27.78ms |

attempts: 20/20 succeeded

metrics:
- `agent_rds_net_connections_accepted_total` = 19
- `client_rds_net_bytes_received_total{via="direct"}` = 26760
- `client_rds_net_bytes_sent_total{via="direct"}` = 77294
- `client_rds_net_connections_opened_total` = 20
- `client_rds_net_datagrams_sent_total{via="direct"}` = 79
- `client_rds_net_paths_seen_total{via="direct"}` = 20


## ping (noq, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 2.28ms | 2.85ms | 6.22ms | 7.64ms | 7.64ms | 3.36ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 3161
- `agent_rds_net_bytes_sent_total{via="direct"}` = 3028
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 12146
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 5
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 15866
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 9092
- `client_rds_net_bytes_sent_total{via="direct"}` = 10512
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 5808
- `client_rds_net_datagrams_sent_total{via="direct"}` = 62
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 1209
- `client_rds_net_selected_path_known` = 1
- `phase_connect_ns` = 20660181


## transfer-receiver-ack-v1 (noq, direct)

throughput: **7.8 MiB/s**

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 3203
- `agent_rds_net_bytes_sent_total{via="direct"}` = 2983
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 12146
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 4
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 17633
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 12914
- `client_rds_net_bytes_sent_total{via="direct"}` = 4297863
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 207778
- `client_rds_net_datagrams_sent_total{via="direct"}` = 2969
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 1689
- `client_rds_net_selected_path_known` = 1
- `phase_connect_ns` = 28673312
- `phase_service_open_ns` = 16053056
- `transfer_completion_ns` = 513370672
- `transfer_verified_bytes` = 4194304

- receiver-ack-v1: byte count + BLAKE3 digest + EOF; payload generation/hash, upload and receipt are timed; connect/OpenTcp are reported separately as phase_* metrics, not folded into throughput; not comparable to historical sender-finish results

## multiconnect (noq, direct-impaired)

impairment: `loss=5.0% delay=50ms jitter=30ms rate=uncapped seed=1`

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 128.54ms | 161.90ms | 1138.19ms | 1173.61ms | 1173.61ms | 260.11ms |

attempts: 20/20 succeeded

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 12125
- `agent_rds_net_bytes_sent_total{via="direct"}` = 16771
- `agent_rds_net_connections_accepted_total` = 14
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 31
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 5
- `agent_rds_net_policy_observed_connections` = 1
- `bench_impair_dropped_datagrams` = 10
- `bench_impair_forwarded_bytes` = 114257
- `bench_impair_forwarded_datagrams` = 171
- `bench_impair_probes` = 2
- `client_rds_net_bytes_received_total{via="direct"}` = 26346
- `client_rds_net_bytes_sent_total{via="direct"}` = 82094
- `client_rds_net_congestion_events_total` = 2
- `client_rds_net_connections_opened_total` = 20
- `client_rds_net_datagrams_lost_total{via="direct"}` = 2
- `client_rds_net_datagrams_sent_total{via="direct"}` = 83
- `client_rds_net_paths_seen_total{via="direct"}` = 20

- impairment probes=2 forwarded=171 dropped=10 bytes=114257

## relay-fallback (noq, relay)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 5.18ms | 8.51ms | 17.16ms | 18.19ms | 18.19ms | 9.72ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="relay"}` = 2707
- `agent_rds_net_bytes_sent_total{via="relay"}` = 3025
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 12143
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 5
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="relay"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 27409
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="relay"}` = 6100
- `client_rds_net_bytes_sent_total{via="relay"}` = 15337
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 5428
- `client_rds_net_datagrams_sent_total{via="relay"}` = 60
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="relay"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 7753
- `client_rds_net_selected_path_known` = 1
- `phase_connect_ns` = 37593267


## impaired (noq, direct-impaired)

impairment: `loss=5.0% delay=50ms jitter=30ms rate=uncapped seed=1`

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 121.39ms | 133.47ms | 465.76ms | 488.66ms | 488.66ms | 169.55ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 9234
- `agent_rds_net_bytes_sent_total{via="direct"}` = 11149
- `agent_rds_net_congestion_events_total` = 1
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 20923
- `agent_rds_net_datagrams_lost_total{via="direct"}` = 8
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 69
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 130293
- `agent_rds_net_selected_path_known` = 1
- `bench_impair_dropped_datagrams` = 8
- `bench_impair_forwarded_bytes` = 20182
- `bench_impair_forwarded_datagrams` = 141
- `bench_impair_probes` = 2
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 9675
- `client_rds_net_bytes_sent_total{via="direct"}` = 10653
- `client_rds_net_congestion_events_total` = 3
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 20233
- `client_rds_net_datagrams_lost_total{via="direct"}` = 6
- `client_rds_net_datagrams_sent_total{via="direct"}` = 78
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 126965
- `client_rds_net_selected_path_known` = 1
- `phase_connect_ns` = 173651472

- impairment probes=2 forwarded=141 dropped=8 bytes=20182

## impaired (noq, relay-impaired)

impairment: `loss=5.0% delay=50ms jitter=30ms rate=uncapped seed=1`

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 240.77ms | 282.03ms | 1048.21ms | 1560.18ms | 1560.18ms | 454.56ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="relay"}` = 7598
- `agent_rds_net_bytes_sent_total{via="relay"}` = 15487
- `agent_rds_net_congestion_events_total` = 4
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 15911
- `agent_rds_net_datagrams_lost_total{via="relay"}` = 12
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 82
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="relay"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 271237
- `agent_rds_net_selected_path_known` = 1
- `bench_impair_dropped_datagrams` = 29
- `bench_impair_forwarded_bytes` = 89563
- `bench_impair_forwarded_datagrams` = 568
- `bench_impair_probes` = 2
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="relay"}` = 7004
- `client_rds_net_bytes_sent_total{via="relay"}` = 18673
- `client_rds_net_congestion_events_total` = 5
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 18498
- `client_rds_net_datagrams_lost_total{via="relay"}` = 10
- `client_rds_net_datagrams_sent_total{via="relay"}` = 88
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="relay"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 309235
- `client_rds_net_selected_path_known` = 1
- `phase_connect_ns` = 1324530560

- impairment probes=2 forwarded=570 dropped=29 bytes=89632

## resolve-connect (noq, discovered)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 20 | 105.00ms | 129.17ms | 165.85ms | 183.59ms | 183.59ms | 134.95ms |

attempts: 20/20 succeeded

metrics:
- `agent_rds_net_active_connections` = 2
- `agent_rds_net_bytes_received_total{via="direct"}` = 102307
- `agent_rds_net_bytes_received_total{via="relay"}` = 13225
- `agent_rds_net_bytes_sent_total{via="direct"}` = 93519
- `agent_rds_net_bytes_sent_total{via="relay"}` = 27880
- `agent_rds_net_connections_accepted_total` = 58
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 173
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 75
- `agent_rds_net_live_paths` = 2
- `agent_rds_net_paths_seen_total{via="direct"}` = 18
- `agent_rds_net_paths_seen_total{via="relay"}` = 3
- `agent_rds_net_policy_observed_connections` = 2
- `client_rds_net_bytes_received_total{via="direct"}` = 120364
- `client_rds_net_bytes_received_total{via="relay"}` = 8381
- `client_rds_net_bytes_sent_total{via="direct"}` = 204418
- `client_rds_net_bytes_sent_total{via="relay"}` = 15877
- `client_rds_net_connections_opened_total` = 20
- `client_rds_net_datagrams_sent_total{via="direct"}` = 294
- `client_rds_net_datagrams_sent_total{via="relay"}` = 23
- `client_rds_net_paths_seen_total{via="direct"}` = 21
- `client_rds_net_paths_seen_total{via="relay"}` = 2
- `client_rds_net_qnt_attempts_total` = 4
- `client_rds_net_qnt_success_total` = 2
- `phase_connect_p50_ns` = 47051285
- `phase_connect_p95_ns` = 61616601
- `phase_first_byte_p50_ns` = 19900050
- `phase_first_byte_p95_ns` = 43822429
- `phase_resolve_p50_ns` = 63742396
- `phase_resolve_p95_ns` = 80971262


## migration (noq, relay-failover-drain)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 5 | 7.41ms | 13.05ms | 31.18ms | 31.18ms | 31.18ms | 15.95ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="relay"}` = 11206
- `agent_rds_net_bytes_sent_total{via="relay"}` = 26525
- `agent_rds_net_connections_accepted_total` = 2
- `agent_rds_net_cwnd_bytes` = 16288
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 62
- `agent_rds_net_live_paths` = 2
- `agent_rds_net_paths_seen_total{via="relay"}` = 2
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 7705
- `agent_rds_net_selected_path_known` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="relay"}` = 5446
- `client_rds_net_bytes_sent_total{via="relay"}` = 14725
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 17909
- `client_rds_net_datagrams_sent_total{via="relay"}` = 43
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="relay"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 9930
- `client_rds_net_selected_path_known` = 1
- `migration_recovery_ns` = 12094877
- `migration_slot_after` = 1
- `phase_connect_ns` = 36199156
- `relay0_forwarded_bytes` = 11707
- `relay0_forwarded_datagrams` = 50
- `relay1_forwarded_bytes` = 17325
- `relay1_forwarded_datagrams` = 87

- relay-failover-drain: slot 0 failed → resumed 12.094877ms on slot 1, 0 probes lost in flight; post-migration RTT from the surviving slot

## migration (noq, relay-failover-kill)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 5 | 6.29ms | 8.10ms | 19.63ms | 19.63ms | 19.63ms | 10.10ms |

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="relay"}` = 2887
- `agent_rds_net_bytes_sent_total{via="relay"}` = 3233
- `agent_rds_net_connections_accepted_total` = 2
- `agent_rds_net_datagrams_sent_total{via="relay"}` = 6
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="relay"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="relay"}` = 5539
- `client_rds_net_bytes_sent_total{via="relay"}` = 15032
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 17164
- `client_rds_net_datagrams_sent_total{via="relay"}` = 43
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="relay"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 9193
- `client_rds_net_selected_path_known` = 1
- `migration_recovery_ns` = 171934506
- `migration_slot_before` = 1
- `phase_connect_ns` = 57759747
- `relay0_forwarded_bytes` = 12240
- `relay0_forwarded_datagrams` = 65
- `relay1_forwarded_bytes` = 15130
- `relay1_forwarded_datagrams` = 39

- relay-failover-kill: slot 1 failed → resumed 171.934506ms on slot 0, 0 probes lost in flight; post-migration RTT from the surviving slot

## calibration (noq, direct-impaired)

impairment: `loss=0.0% delay=0ms jitter=0ms rate=10Mbps seed=1`

throughput: **0.9 MiB/s**

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 3819514
- `agent_rds_net_bytes_sent_total{via="direct"}` = 49414
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 19499
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 1279
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 11687
- `agent_rds_net_selected_path_known` = 1
- `bench_impair_forwarded_bytes` = 4353287
- `bench_impair_forwarded_datagrams` = 4407
- `bench_impair_probes` = 2
- `calibration_expected_bytes_s` = 1250000
- `calibration_measured_bytes_s` = 936955
- `calibration_ratio_milli` = 749
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 54707
- `client_rds_net_bytes_sent_total{via="direct"}` = 4298580
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 7551
- `client_rds_net_datagrams_sent_total{via="direct"}` = 2969
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 5637
- `client_rds_net_selected_path_known` = 1
- `phase_connect_ns` = 29366373
- `phase_service_open_ns` = 33458770
- `transfer_completion_ns` = 4476525857
- `transfer_verified_bytes` = 4194304

- impairment probes=2 forwarded=4408 dropped=0 bytes=4353324
- receiver-ack-v1: byte count + BLAKE3 digest + EOF; payload generation/hash, upload and receipt are timed; connect/OpenTcp are reported separately as phase_* metrics, not folded into throughput; not comparable to historical sender-finish results
- no --rate-mbps given; calibrated at the built-in 10 Mbps

## recovery (noq, direct-impaired)

impairment: `loss=15.0% delay=50ms jitter=0ms rate=uncapped seed=1`

throughput: **0.6 MiB/s**

metrics:
- `agent_rds_net_active_connections` = 1
- `agent_rds_net_bytes_received_total{via="direct"}` = 1102385
- `agent_rds_net_bytes_sent_total{via="direct"}` = 13626
- `agent_rds_net_connections_accepted_total` = 1
- `agent_rds_net_cwnd_bytes` = 19339
- `agent_rds_net_datagrams_sent_total{via="direct"}` = 163
- `agent_rds_net_live_paths` = 1
- `agent_rds_net_paths_seen_total{via="direct"}` = 1
- `agent_rds_net_policy_observed_connections` = 1
- `agent_rds_net_rtt_us` = 16499
- `agent_rds_net_selected_path_known` = 1
- `bench_impair_dropped_datagrams` = 44
- `bench_impair_forwarded_bytes` = 2170670
- `bench_impair_forwarded_datagrams` = 1719
- `bench_impair_probes` = 2
- `client_rds_net_active_connections` = 1
- `client_rds_net_bytes_received_total{via="direct"}` = 15873
- `client_rds_net_bytes_sent_total{via="direct"}` = 2218685
- `client_rds_net_congestion_events_total` = 36
- `client_rds_net_connections_opened_total` = 1
- `client_rds_net_cwnd_bytes` = 59979
- `client_rds_net_datagrams_lost_total{via="direct"}` = 45
- `client_rds_net_datagrams_sent_total{via="direct"}` = 1536
- `client_rds_net_live_paths` = 1
- `client_rds_net_paths_seen_total{via="direct"}` = 1
- `client_rds_net_policy_observed_connections` = 1
- `client_rds_net_rtt_us` = 2497
- `client_rds_net_selected_path_known` = 1
- `recovery_delay_ms` = 50
- `recovery_imposed_window_ns` = 1500990737
- `recovery_loss_milli` = 150
- `transfer_completion_ns` = 3502335441
- `transfer_verified_bytes` = 2097152

- path loss imposed mid-transfer at ~1/3 payload (loss=0.15, delay=50ms on client egress, restored after 1.500990737s); verified receipt still arrived; 44 datagrams dropped under the imposed loss

