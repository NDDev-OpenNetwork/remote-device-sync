# Observed path telemetry — 2026-09-25

Scope: W0 measurement correctness, W3.6 path policy, W10.1 observability.
Source base: `b2f76dc7ee8f312d5c995101786a88c4c908a917`. Modified source hashes, toolchain and
check commands are in the [machine receipt](rds-path-telemetry-20260925-data.json). Linux x86_64 only.

## Reproduced failures

Before implementation, an actual validated replacement carried data while
`current_path_stats()` still reported path zero. A separate owned relay process
forwarded authenticated stream/datagram traffic through its only path, but that
path was labeled direct. Both regressions failed with exit code 101.

## Change

Noq now shares policy-observed weak path metadata with the connection facade.
The initial handshake is seeded synchronously; Established events add later
paths. Live checks exclude retained closed-path statistics and pending probes.
There is no ID scan or ceiling. Relay classification uses the real synthetic
address, and selection reflects successfully applied policy state.

Coverage explicitly distinguishes a policy observation from an engine snapshot.
Lost events remain visible and suppress a selected-path estimate. Cancellation
clears the observer, even before its first poll. Pacing and metrics never
substitute a historical or arbitrary path. RTT/cwnd/validity are stored together;
sampler baseline storage follows concurrent observed paths. See the
[contract](../path-telemetry.md) for limits and metric semantics.

## Validation

All seven final checks passed: workspace/shared formatting, workspace Clippy
in default, Linux X11 and all-feature configurations with `-D warnings`,
workspace tests, and expanded all-feature net/relay/agent/CLI/server tests.
Workspace: **419 passed**, 1 ignored, 72 targets.
Expanded: **226 passed**, 0 ignored, 43 targets.

The actual-path campaign opens 70 paths sequentially, crosses ID 63, migrates
away from path zero and exchanges a datagram. A subscribed but unpolled observer
really overflows the engine event buffer, then reports sticky loss and unknown
selection. Tests check loss-counter deltas, gauge release, running/unpolled
observer cancellation, post-close emptiness and ID conversion boundaries.
Both real binary restart fixtures assert relay-only observed bytes and labels.

An earlier matrix attempt stopped when its build directory disappeared; it is
not counted as successful validation. The final complete matrix used a fresh,
isolated target directory. Cargo-deny is unavailable. Native macOS is pending.

## Limits

Noq observations are not complete or atomic, even with zero reported loss.
Full reconciliation, lossless short/retired-path accounting, weak sampler
ownership and per-session metrics remain open. Counters describe inner QUIC
traffic and omit outer relay encapsulation. Selection is a policy preference,
not a per-packet trace. Historical Noq path evidence needs new qualification
runs. No benchmark speedup, deployed service readiness or wave closure is claimed.
