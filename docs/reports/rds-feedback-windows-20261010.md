# Desktop feedback observation windows — 2026-10-10

Baseline: `c3dc9c18e5881c24a1519a4493c4af6b6e19521f`. Its
[macOS CI job](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/37974406899)
failed two independent checks: recovered frame delivery reduced bitrate from
4,000,000 to 2,800,000 after a measured 382 ms transport hold, and the Iroh
standby fixture did not establish both required paths within five seconds.
Ubuntu passed. The improved heartbeat cohort was not the failing check.

## Reproduced feedback defect

The bitrate controller samples delivery pressure every 250 ms and requires
successive observations for sustained pressure. Its timer previously produced
an immediate first observation and used `MissedTickBehavior::Skip`. After a
scheduler pause, Skip can place the next observation very near the late one.
Two paused-time regressions failed against the original timer: the first
observation occurred at zero elapsed time, and observations after a 740 ms
pause were only 10 ms apart.

The timer now starts after its first 250 ms window and uses Delay to schedule
the next window from a substantially delayed observation. Both regressions
pass. This follows the documented [Tokio interval](https://docs.rs/tokio/1.48.0/tokio/time/fn.interval.html)
and [missed-tick policies](https://github.com/tokio-rs/tokio/blob/tokio-1.48.0/tokio/src/time/interval.rs).
The same implementation was inspected in the locked Tokio 1.53.1 source.
Tokio tolerates small timer jitter; this is not a hard real-time guarantee.

Ten additional runs of the two real Noq frame-delivery checks passed: short
holds preserved bitrate and sustained holds still reduced it. Test output now
retains the controller's actual reduction reason. This fixes a reproduced
sampling defect; the original CI output did not record that reason, so it does
not prove that timer compression caused the particular 382 ms failure.

## Standby investigation remains open

The original error said UDP never validated, but its condition required both
UDP and a custom path. It could not distinguish the missing path. The fixture
now reports both endpoints' path IDs, selection, RTT, advertised addresses and
known peer addresses on failure. Its five-second deadline and transport
behavior are unchanged.

Twelve standalone runs and all 112 network library tests together passed
locally before any transport change. A separate check of address publication
also passed the old fixture; it did not reproduce the suspected setup race.
These results do not close the CI failure. Retain the next exact-head CI
failure, if any, and use the new state evidence before choosing a fix. Do not
raise the deadline or change path selection based only on the old message.

At `47e06a5`, macOS arm64 workspace validation passed formatting, strict
all-target Clippy, 925 tests (2 explicitly ignored) and `cargo deny check`.
Both native packaging jobs and supply-chain jobs passed in PR146; the full
Linux/macOS test matrix and Rust CodeQL are still running at this observation.
These checks do not explain the original standby failure.
This increment does not close the desktop wave, native acceptance or topology
parity, and does not claim installed-artifact qualification.
