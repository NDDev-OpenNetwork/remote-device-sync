# Managed native control progress during decode

Scope: W6.3/W6.4 and diagnostic W6.7 remediation; native latency/stability
acceptance remains open.

The native managed viewer previously awaited `RelayDecoder::push_bounded`
inside a selected receive handler. While that future was pending, the same
loop could not dispatch input, send heartbeat or observe close. The outer
five-second decode timeout bounded the wait but still coupled input latency
to codec scheduling. This source-level defect is separate from any physical
network outage.

The corrected owner concurrently polls two whole, retained futures: outbound
input/heartbeat/watchdog and inbound event/decode. It adds no frame queue,
blocking task or codec permit. H.264 reference order and the one pending raw
presentation frame remain unchanged. Two-second control writes and the
15-second decoded-progress watchdog retain their bounds. A decode failure,
write failure, remote end, close or cancellation ends both futures and the
desktop IPC by EOF. No Finished frame follows a potentially canceled partial
control write. The manager's authenticated connection and unrelated streams
remain owned by the manager.

Every reconnect warning now captures metadata about frame/UI ages, network
and render stages, outstanding input acknowledgements, control RTT and
occlusion before changing status. It contains no screen/text/input content.

## Functional regression boundary

`desktop::control::tests` drives the production dispatch/ownership functions
with a blocked synthetic media future and real bounded, framed duplex I/O.
It verifies:

- Ordered Alt/arrow press and release controls reach the reader while decode
  is still held; two heartbeat frames arrive within 1.2 seconds of simulated
  time. Close drops the held media future within one simulated millisecond.
- A permanently held media future cannot suppress the 15-second progress
  watchdog.
- A blocked control write fails at two seconds, and cancellation immediately
  drops both pending legs.
- Actual decoded-progress notifications renew the watchdog through 30 seconds
  without reconstructing the control future.

These paused-clock fixtures exercise dispatch, framing and ownership. They
do not use a real OS input sink, blocking codec, GPU or physical display.
Incoming ACK/heartbeat observation can still wait behind decode in the existing
bounded IPC receive route; this change does not claim otherwise.

## Reproduction

```sh
cargo fmt --check
cargo test --locked -p rds-cli --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

The macOS all-feature CLI lane passed 42 tests across 12 groups, with zero
failures and one explicitly ignored external OpenSSH/account fixture. Its
desktop library group passed nine tests (four new dispatch fixtures, three
existing reconnect fixtures and two logging fixtures), with zero failures or
ignored cases. Full-workspace/all-target/all-feature strict Clippy and formatting
also passed. Linux/build results are recorded with the candidate's PR.
No physical latency, quality or long-soak gate is closed by these checks.
