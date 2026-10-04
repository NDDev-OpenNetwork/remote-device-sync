# Desktop stage observation — 2026-10-05

W6.8/WS5 and W10.1 follow-up; no acceptance gate is closed here.

A one-second age since submission can be normal on a damage-driven idle screen.
The flight recorder previously classified this as a visible renderer stall.
Native snapshots now expose the local age of decoded work awaiting submission.
Newer pending images cannot reset that age and hide a blocked renderer. A
successful submission clears it, retaining the queued time of an image that
arrived concurrently with the draw. Occlusion remains a separate condition.

Input diagnostics now observe managed IPC receipt, bounded network-queue
admission, and control-write start/completion. Existing dispatch/ACK, server
injection, frame sequence and complete-payload proof records remain. These new
records contain sequence numbers and local durations, with no input content.
Local write success is neither server execution nor native presentation.

Research checked against primary sources:

- [Iroh 1.2 endpoint documentation](https://docs.rs/iroh/1.2.0/iroh/endpoint/struct.Endpoint.html)
  recommends sharing an application endpoint and explains the explicit address
  hints used for direct connectivity. RDS retains its managed endpoint ownership.
- [Iroh relay documentation](https://docs.rs/iroh/latest/iroh/) describes relay
  traffic over TCP. QUIC stream priorities cannot eliminate the outer carrier's
  ordered-delivery waits; a QUIC loss counter alone is not carrier-loss evidence.
- [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221.html) separates application
  processing from transport acknowledgements and retains congestion control for
  datagrams. Switching to unreliable delivery is not itself a capacity or
  stability fix and requires reference/repair qualification.
- [GStreamer queue documentation](https://gstreamer.freedesktop.org/documentation/coreelements/queue.html)
  documents how bounded queues can block upstream. RDS keeps its bounded queues
  and independent input/event/media futures; there is no GStreamer dependency.

Deterministic regressions cover a long idle interval, a fresh update after idle,
successive pending replacements, and an image arriving during submission.
Local macOS full-workspace tests with both native desktop features passed, as
did default and native-feature strict workspace Clippy, formatting and cargo-deny.
Installed-device latency and sustained connection acceptance are separate from
these classification tests. Private runtime evidence belongs to the consumer.
