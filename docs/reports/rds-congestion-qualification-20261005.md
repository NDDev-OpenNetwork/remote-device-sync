# Explicit congestion qualification — 2026-10-05

W3/WS2 transport observation and W6.8 follow-up. This increment supplies a
measured-comparison setting; it does not establish a preferred controller or
close a desktop/transport acceptance gate.

The optional version-1 endpoint setting `congestion_control` accepts `bbr3` or
`cubic`. BBRv3 remains the default and its field is omitted from serialized
defaults. Both Iroh and owned Noq instantiate the same pinned engine factory.
No identity, authorization, path-kind, packetization, priority, window, timeout
or wire setting changes along with it. Old strict binaries refuse the explicit
new field and require upgrade before use.

Regression validation distinguishes the actual controller types produced by
the factories, checks legacy omission and explicit roundtrip/lowering, rejects
unknown/duplicate selections and transfers 256 KiB in each direction over real
connections on both backends with each controller. These are interoperability
checks, not real-network capacity or visible latency evidence.
Local config/facade unit tests and the four configuration integration tests
passed with owned transport enabled, along with strict native/owned-feature
workspace Clippy and formatting. Installed comparisons remain required.

The maintained [Noq controller API](https://docs.rs/noq-proto/1.3.0/noq_proto/congestion/index.html)
exposes both factories. The historical
[BBRv3 accounting correction](https://github.com/n0-computer/noq/pull/689)
replaced an earlier proposed patch; the pinned version's source must be inspected
before treating old issues as present defects. A separate
[Iroh performance report](https://github.com/n0-computer/iroh/issues/4286)
reported the opposite controller outcome on an older LAN stack. Such reports
motivate controlled qualification, not a universal default change.

Private device/route results and actual adoption remain the consumer's evidence.
