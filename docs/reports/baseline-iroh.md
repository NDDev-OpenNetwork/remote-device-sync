# rds bench suite — 1790020370

commit: `a763f5c`

## handshake (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 22.50ms | 31.04ms | 53.19ms | 57.32ms | 65.92ms | 34.05ms |

attempts: 100/100 succeeded


## ping (iroh, direct)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 1.23ms | 2.16ms | 6.64ms | 9.87ms | 22.92ms | 2.99ms |


## transfer (iroh, direct)

throughput: **16.4 MiB/s**


## multiconnect (iroh, direct-impaired)

impairment: `loss=5.0% delay=50ms jitter=30ms rate=uncapped seed=1`

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 138.33ms | 171.37ms | 1197.64ms | 1316.10ms | 1329.60ms | 258.85ms |

attempts: 100/100 succeeded

- proxy: forwarded=810 dropped=51 bytes=589502

## relay-fallback (iroh, relay)

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 1.15ms | 1.96ms | 5.97ms | 10.51ms | 14.65ms | 2.53ms |


## impaired (iroh, direct-impaired)

impairment: `loss=5.0% delay=50ms jitter=30ms rate=uncapped seed=1`

| n | min | p50 | p95 | p99 | max | mean |
|---|-----|-----|-----|-----|-----|------|
| 100 | 1.07ms | 2.05ms | 6.91ms | 13.83ms | 14.28ms | 2.75ms |

- proxy: forwarded=26 dropped=1 bytes=11221

