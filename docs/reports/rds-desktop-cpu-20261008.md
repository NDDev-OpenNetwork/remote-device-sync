# Desktop CPU work — 2026-10-08

W6.1 software-capture follow-up. The X11 producer now converts/encodes a
synchronous borrowed view of the completed MIT-SHM mapping. Native-resolution
capture no longer allocates and copies an 8,294,400-byte BGRA staging frame.
Downscaling reads the same view and allocates only its smaller output. The
public owned `Capturer::capture` and `Encoder::encode` contracts remain intact;
plain GetImage supplies the view from its owned reply without another copy.
No new dependency, unsafe block, codec setting, FPS cap or wire change is added.

The exclusive capturer borrow spans the entire callback. No subsequent
ShmGetImage request can overwrite its segment before synchronous conversion and
encode return; encoded bytes retain their separate ownership. Native tests
check mapping-address identity, different paints, retained owned snapshots and
the plain GetImage fallback. Codec tests compare owned and borrowed input byte
for byte across explicit IDR recovery; rate-control and reference tests remain.

## Measurement

The [frozen Rust input](rds-capture-conversion-20261008.rs) runs five alternating
pairs of 600 Full HD conversions, reusing the I420 destination in both modes.
The copied mode calls the prior `Bytes::copy_from_slice` immediately before the
same stride-aware converter; borrowed mode reads the source directly. The
[raw data](rds-desktop-cpu-20261008-data.json) retains every pair.

| Isolated stage | Median per frame |
| --- | ---: |
| Owned capture copy + conversion | 3.487 ms |
| Borrowed capture + conversion | 1.848 ms |

This stage takes 47.0% less elapsed time in this shared Linux VM measurement. It is not a
47% reduction in whole-agent CPU, end-to-end latency or physical display time.
The synthetic source is fixed, the run measures elapsed time, and concurrent
host work is not controlled. Live software encode remains the dominant cost.

Reproduce with Rust 1.98.1 and this checkout's locked release dependency
artifacts (substitute the actual Cargo artifact hashes):

```sh
rustc --edition=2024 -C opt-level=3 docs/reports/rds-capture-conversion-20261008.rs \
  --extern bytes=target/release/deps/libbytes-<hash>.rlib \
  --extern openh264=target/release/deps/libopenh264-<hash>.rlib \
  -L dependency=target/release/deps -o /tmp/rds-capture-conversion
/tmp/rds-capture-conversion
```

An allocation-recycling-only candidate did not improve the paired copy run and
is absent from the final implementation. Disabling scene-change detection is
also ineffective: OpenH264's screen-content `ParamValidation` forces it back on.
[Upstream configuration validation](https://github.com/cisco/openh264/blob/master/codec/encoder/core/src/encoder_ext.cpp)
and the exact openh264-sys2 0.9.8 source confirm that behavior. Medium codec
complexity remains; lowering it would be a separate quality/bitrate decision.

## Qualification boundary

The macOS x11 feature lane checks the codec and scaling code; native X11 work
must pass on Linux under a dedicated Xvfb display. Required PR checks cover
both supported OSes and Linux native fixtures. Installed runtime receipts and
private identities belong to the estate. This increment does not close the
broader hardware-codec, geometry, quality or glass-to-glass gates.
