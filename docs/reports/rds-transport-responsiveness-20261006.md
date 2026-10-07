# Iroh actor responsiveness baseline — 2026-10-06

W1/W10 resilience increment: require Iroh/relay1.3 and lock Iroh/base/relay1.3.0.
Noq remains1.3.0. No new dependency, transport setting, wire message, authority,
stream-priority or input-ordering change.

[Iroh1.3](https://github.com/n0-computer/iroh/releases/tag/v1.3.0) includes
[4512](https://github.com/n0-computer/iroh/pull/4512), which keeps a remote actor
responsive while an Initial/datagram send waits behind a blocked relay queue.
The owned bounded send work uses concurrent selected destinations and a deadline;
QUIC retransmits Initials. Upstream regression covers the full real relay queue,
responsive RemoteInfo, direct delivery, expiry, queue limit and shutdown. The
published1.3.0 source contains the bounded futures and three-second send deadline.

This addresses a known setup/recovery risk. It does not establish the cause of a
steady-state input pause or close native latency/stability acceptance. A small
control/frame backlog must still be joined across application, transport, native
input and presentation, with qualified clocks for cross-host comparisons.

Formatting/diff checks pass. macOS full workspace desktop-feature tests are in
progress; cross-platform CI, strict Clippy and installed qualification remain
required. No installed endpoint or protected desktop service was changed by this
source increment.
