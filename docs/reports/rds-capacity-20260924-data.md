# rds bench suite — 1790265000

commit: `3d4ad08`

## directory-fill (redb, local-file)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 4096 | 10.95ms | 30.09ms | 73.48ms | 110.05ms | 279.15ms | 35.28ms |

attempts: 4096/4096 succeeded

metrics:
- `database_limit_bytes` = 268435456
- `identities` = 4096
- `observed_allocated_peak_bytes` = 33792000
- `observed_database_peak_bytes` = 36704256

- Build debug assertions: true. Use the paired receipt for compiler/profile and working-tree provenance.
- Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.
- File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.
- Large fixture: 32 IPv6 candidates, 8 raw 512-byte relay origins, all six services; signed payload 4805..4805 bytes. Addresses are synthetic and never dialed.

## directory-renew-1 (redb, local-file)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 4096 | 13.13ms | 27.44ms | 56.67ms | 85.30ms | 227.28ms | 31.29ms |

attempts: 4096/4096 succeeded

metrics:
- `database_limit_bytes` = 268435456
- `identities` = 4096
- `observed_allocated_peak_bytes` = 33800192
- `observed_database_peak_bytes` = 36704256

- Build debug assertions: true. Use the paired receipt for compiler/profile and working-tree provenance.
- Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.
- File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.

## directory-renew-2 (redb, local-file)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 4096 | 11.13ms | 25.97ms | 59.42ms | 90.02ms | 197.12ms | 30.27ms |

attempts: 4096/4096 succeeded

metrics:
- `database_limit_bytes` = 268435456
- `identities` = 4096
- `observed_allocated_peak_bytes` = 33824768
- `observed_database_peak_bytes` = 36704256

- Build debug assertions: true. Use the paired receipt for compiler/profile and working-tree provenance.
- Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.
- File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.

## directory-delete-half (redb, local-file)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 2048 | 13.45ms | 26.39ms | 41.96ms | 51.27ms | 64.31ms | 27.66ms |

attempts: 2048/2048 succeeded

metrics:
- `database_limit_bytes` = 268435456
- `identities` = 4096
- `observed_allocated_peak_bytes` = 33824768
- `observed_database_peak_bytes` = 36704256

- Build debug assertions: true. Use the paired receipt for compiler/profile and working-tree provenance.
- Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.
- File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.

## directory-reactivate-half (redb, local-file)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 2048 | 10.29ms | 17.27ms | 32.37ms | 47.17ms | 76.40ms | 19.09ms |

attempts: 2048/2048 succeeded

metrics:
- `database_limit_bytes` = 268435456
- `identities` = 4096
- `observed_allocated_peak_bytes` = 33824768
- `observed_database_peak_bytes` = 36704256

- Build debug assertions: true. Use the paired receipt for compiler/profile and working-tree provenance.
- Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.
- File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.

## directory-reopen (redb, local-file)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 4 | 1541.51ms | 8997.12ms | 15977.29ms | 15977.29ms | 15977.29ms | 9454.61ms |

attempts: 4/4 succeeded

metrics:
- `database_limit_bytes` = 268435456
- `identities` = 4096
- `observed_allocated_peak_bytes` = 33824768
- `observed_database_peak_bytes` = 36704256

- Build debug assertions: true. Use the paired receipt for compiler/profile and working-tree provenance.
- Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.
- File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.

