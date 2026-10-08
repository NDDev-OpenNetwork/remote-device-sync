# Directory snapshot benchmark

Source: `f1a79c77688572d08c04d95a8e2fafeed9c50c90`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Entries | Samples | Parts | Reference median ms | Borrowed median ms |
|---:|---:|---:|---:|---:|
| 1024 | 10 | 32 | 6.315 | 1.240 |
| 8192 | 10 | 256 | 44.264 | 9.014 |
| 32768 | 10 | 1024 | 171.551 | 34.574 |

Every sample verifies identical part count, serialized byte count and framed payload digest.

Synthetic in-process release-build wall time; does not measure installed CPU, filesystem throughput, network, or remote recursive service acceptance. Reference is preserved algorithm code, not a second deployed binary. No hardware-independent speed threshold.
