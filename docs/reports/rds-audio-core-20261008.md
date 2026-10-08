# Audio core foundation — 2026-10-08

This wave makes the audio library a real bounded core while keeping the public
capability truthful: no device source, playback sink, agent admission or viewer
playout is advertised yet.

Implemented in `rds-audio`:

- validated Opus sample rates, channel counts and all standard frame durations, including 2.5 ms;
- libopus encode/decode with a 1275-byte per-frame ceiling and 20 ms
  default frames; multi-frame packetization is intentionally not exposed;
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


## Contract correction — 2026-10-09

The initial core accepted `2` as if it were a valid two-millisecond frame.
Opus defines `2.5`, `5`, `10`, `20`, `40` and `60` ms durations; the corrected
API exposes these through `FrameDuration`, computes samples from microseconds,
and keeps the original whole-millisecond helper source-compatible while
rejecting the invalid `2` value. Tests cover every supported sample rate and
duration. The core wire record still stores capture time in whole milliseconds,
so conversion documents the intentional sub-millisecond truncation.
