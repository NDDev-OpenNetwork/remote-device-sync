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

## Reorder and loss contract

`JitterBuffer` retains at most its configured capacity (1–64 packets). On
overflow it discards the oldest of the retained and incoming packets. An older
arrival cannot displace newer retained audio. Duplicates preserve the original
packet. `accepted` counts actual insertions, `overflow` counts drops, and `late`
includes queued packets removed when establishing an explicit playout floor.
All diagnostic counters saturate.

`start_at` may establish the floor once, before the first pop. Without it, the
first pop starts at the lowest retained sequence. After starting, earlier
packets are late. `target_delay` is a count of queued successor packets tolerated
before declaring a hole; it is not a clock, prebuffer or a bound on wall time.
The device owner must eventually supply playout deadlines for complete silence.

A hole no larger than capacity emits exactly one `Gap` for each missing frame,
allowing one fixed-duration PLC operation per pop. A larger hole emits one
`Discontinuity { from, to }`, advances to the first retained packet and counts
the entire missing range. The caller must reset decoder/playout state instead
of synthesizing an unbounded number of PLC frames. This is an additive library
enum variant, so future callers must handle it explicitly; no live service or
wire message uses the enum yet.

`u64::MAX` is reserved for the exhausted position. Packets and explicit starts
at that value are refused, the encoder fails before encoding another frame,
and playout never wraps to accept an old epoch. Starting a fresh audio session
is the explicit recovery path.

Validation: `cargo test -p rds-audio --test jitter_contract`. Fixtures cover
all permutations of four arrivals in a two-packet buffer, consecutive losses,
late arrivals, explicit floors, huge jumps, sequence exhaustion and saturated
counters. This policy is bounded core behavior, not a completed adaptive jitter
or audio-device pipeline.
