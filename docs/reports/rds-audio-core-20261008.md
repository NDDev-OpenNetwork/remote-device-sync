# Audio core foundation — 2026-10-08

This wave makes the audio library a real bounded core while keeping the public
capability truthful: no device source, playback sink, agent admission or viewer
playout is advertised yet.

Implemented in `rds-audio`:

- validated Opus sample rates, channel counts and standard frame durations;
- libopus encode/decode with a 1275-byte packet ceiling and 20 ms
  default frames;
- deterministic sequence/timestamp/sample-count conversion to
  `rds_core::AudioFrame`;
- malformed-packet rejection;
- bounded sequence reorder buffering with duplicate, late and overflow counters;
- explicit gap reporting for decoder packet-loss concealment;
- six unit tests covering format bounds, encode/decode/PLC, reorder, loss,
  overflow and wire validation.

The codec dependency is pinned to `opus 0.4.0` with bundled `opusic-sys`/
libopus (MIT/Apache-2.0 plus BSD-3-Clause). The official libopus reference
implementation is the normative interoperability boundary; the Rust crate is a
thin safe binding and the bundled build keeps installation reproducible. The
core owns no callback, async task, socket or device handle, so platform adapters
cannot block the transport or bypass the shared bounds.

Not claimed by this report: microphone/system-audio capture, CoreAudio or
PipeWire adapters, playback, permission lifecycle, audio grants, an audio
uni-stream, A/V clock synchronization or installed-device audio acceptance.
Those are the next completion-plan workstream and keep `service:audio` in the
`stub` state until their end-to-end gate passes.
