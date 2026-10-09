# Audio foundation benchmark

Source: `93ae814121a0bacdc4d79539dc47c793f83bfc51`; tree: `730e6581b0dbd32fd16f33b5d32984856cf665b6`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Rate | Channels | Frames | PLC | Encode p50/p95/p99 µs | Decode/PLC p50/p95/p99 µs | Peak packets |
|---:|---:|---:|---:|---|---|---:|
| 8000 | 1 | 500 | 10 | 77.9/154.2/282.1 | 2.8/6.1/12.5 | 4 |
| 8000 | 2 | 500 | 10 | 76.3/105.8/151.7 | 14.5/19.6/77.6 | 4 |
| 12000 | 1 | 500 | 10 | 105.9/151.5/187.6 | 3.4/5.7/9.9 | 4 |
| 12000 | 2 | 500 | 10 | 82.6/109.5/246.9 | 17.2/25.4/77.5 | 4 |
| 16000 | 1 | 500 | 10 | 72.6/77.8/108.9 | 9.8/11.3/52.6 | 4 |
| 16000 | 2 | 500 | 10 | 98.6/144.7/328.0 | 17.8/30.7/75.8 | 4 |
| 24000 | 1 | 500 | 10 | 68.5/99.9/131.9 | 12.2/26.0/51.7 | 4 |
| 24000 | 2 | 500 | 10 | 98.4/129.6/178.5 | 19.5/25.9/74.0 | 4 |
| 48000 | 1 | 500 | 10 | 80.9/89.5/103.0 | 13.3/16.2/47.8 | 4 |
| 48000 | 2 | 500 | 10 | 127.0/138.7/160.9 | 23.1/25.7/68.0 | 4 |

Synthetic synchronous release-build wall time, 20 ms frames and 440 Hz PCM; groups of four arrive in reverse order, with frame 21 of every 50 omitted unless it is the final packet. Every output has the exact PCM length and finite samples, every loss has one PLC interval, and the buffer never exceeds four packets. No timer, device callback, network, optical latency, A/V drift or perceptual-quality measurement. Decode timing includes packet validation/PCM allocation or PLC. No machine-independent speed threshold.
