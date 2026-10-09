# Audio foundation contract

`rds-audio` is a synchronous library, not an advertised audio service. It owns
bounded Opus packet validation, fixed 20 ms encoding/decoding and a bounded
reorder buffer. There is no device callback, transport task, playout clock,
microphone authorization or A/V synchronization yet. Those remain W9.1 in the
[stability plan](stability-plan-20261009.md).

## Packet profile

The codec accepts mono/stereo PCM at 8, 12, 16, 24 or 48 kHz. `FrameDuration`
represents all six standard durations including 2.5 ms; the encoder, decoder
and reorder buffer currently use 20 ms. The wire conversion helper can validate
another standard duration explicitly supplied by a future negotiated caller.

RDS accepts one Opus frame per packet. The frame limit is 1275 bytes; the packet
limit is 1276 bytes, including a TOC byte. Alternate single-frame encodings and
padding are accepted only within that packet limit. Multi-frame packets are
refused even when their aggregate duration matches. Zero-byte frames with a
valid TOC are allowed; an empty packet is reserved for internal PLC.

Wire conversion, buffer admission and decode use the same libopus packet
parser and duration check. A caller's `samples` integer does not authenticate
the compressed duration. Wrong duration or malformed framing is rejected before
allocating PCM or mutating decoder history; failed admission cannot evict queued
audio. Compressed mono is valid for stereo PCM output because libopus owns
channel conversion. Compression validity still belongs to the decoder, not
merely to the framing parser.

Encoded sequence and sample counts remain exact. The existing core wire record
has millisecond timestamps, so conversion truncates a microsecond remainder and
rejects overflow when converting back. No remote wire tag or layout changed.

The framing distinction follows [RFC 6716 §§3.1–3.2](https://www.rfc-editor.org/rfc/rfc6716.html#section-3.1).
The [libopus API](https://opus-codec.org/docs/opus_api-1.5.pdf) supplies parsing,
stateful decode and packet loss concealment. The one-frame profile and packet
cap are RDS policy, not a restriction on general Opus interoperability.

Validation: `cargo test -p rds-audio --lib --test packet_contract`, including
real encoding/decoding at every supported PCM format and unchanged decoder
history after a forged duration. Device/network qualification is separate.
