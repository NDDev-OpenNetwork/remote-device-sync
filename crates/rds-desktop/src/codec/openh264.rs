//! H.264 codec over OpenH264: Annex-B, constrained baseline, no B-frames.
//!
//! Software encode is the portability floor; hardware encoders plug in
//! behind the same `Encoder`/`Decoder` traits later.

use bytes::Bytes;
use openh264::decoder::{Decoder as OhDecoder, DecoderConfig};
use openh264::encoder::{
    BitRate, EncodedBitStream, Encoder as OhEncoder, EncoderConfig, FrameRate, IntraFramePeriod,
    RateControlMode, UsageType,
};
use openh264::formats::{BGRA8Source, RGB8Source, RGBSource, YUVBuffer, YUVSource};
use openh264::{Error as OhError, OpenH264API};
use rds_core::Codec;

use crate::{Decoder, DesktopError, EncodedFrame, Encoder, RawFrame};

/// Real-time OpenH264 encoder feeding `EncodedFrame`s.
pub struct H264Encoder {
    inner: OhEncoder,
    /// Rate the live encoder is configured for.
    bitrate: u64,
    /// Requested rate waiting for the next encode. Native SetOption updates
    /// retain reference pictures and rate-control history instead of emitting
    /// a large IDR on every adaptation step.
    pending_bitrate: Option<u32>,
    dimensions: Option<(u32, u32)>,
    fps: f32,
    want_idr: bool,
    started: std::time::Instant,
    last_timestamp: Option<u64>,
    /// Recycled I420 input buffer — a 1080p frame is ~3 MiB, so a fresh
    /// allocation per frame at 60 fps is ~190 MB/s of pure alloc churn.
    /// Rebuilt only when frame dimensions change.
    yuv_buf: Option<YUVBuffer>,
}

impl H264Encoder {
    pub fn new(bitrate_bps: u64, fps: f32) -> Result<Self, DesktopError> {
        let bitrate_bps = bitrate_bps.clamp(1, i32::MAX as u64);
        let inner = Self::build(bitrate_bps as u32, fps)?;
        Ok(Self {
            inner,
            bitrate: bitrate_bps,
            pending_bitrate: None,
            dimensions: None,
            fps,
            want_idr: true,
            started: std::time::Instant::now(),
            last_timestamp: None,
            yuv_buf: None,
        })
    }

    fn build(bitrate_bps: u32, fps: f32) -> Result<OhEncoder, DesktopError> {
        let config = EncoderConfig::new()
            // Screen content: the desktop is text and sharp edges, not
            // camera footage — this tunes QP/mode decisions for it.
            .usage_type(UsageType::ScreenContentRealTime)
            // Unsupported for screen content — OpenH264 disables them
            // with a warning; set explicitly instead.
            .adaptive_quantization(false)
            .background_detection(false)
            .rate_control_mode(RateControlMode::Timestamp)
            .bitrate(BitRate::from_bps(bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(fps))
            // ~8s at 30fps: a bound on how long a client that missed
            // every resync hint stays undecodable.
            .intra_frame_period(IntraFramePeriod::from_num_frames(240))
            .skip_frames(true);
        let api = OpenH264API::from_source();
        OhEncoder::with_api_config(api, config).map_err(|e| DesktopError::Encode(e.to_string()))
    }
}

impl Encoder for H264Encoder {
    fn encode(&mut self, frame: &RawFrame) -> Result<EncodedFrame, DesktopError> {
        self.encode_timed(frame, self.started.elapsed().as_millis() as u64)
    }
    fn request_idr(&mut self) {
        self.want_idr = true;
    }
    fn set_bitrate(&mut self, bps: u32) {
        let bps = u64::from(bps);
        if bps == 0 || bps == self.bitrate {
            return;
        }
        self.pending_bitrate = Some(bps.min(i32::MAX as u64) as u32);
    }
}

impl H264Encoder {
    fn encode_timed(
        &mut self,
        frame: &RawFrame,
        now_ms: u64,
    ) -> Result<EncodedFrame, DesktopError> {
        let dimensions = (frame.width, frame.height);
        let target = self
            .pending_bitrate
            .take()
            .map(u64::from)
            .unwrap_or(self.bitrate);
        if self.dimensions != Some(dimensions) {
            // The wrapper initializes lazily and restores its construction
            // config on geometry changes. Build with the current rate only at
            // that genuine reference discontinuity; never call raw options on
            // an uninitialized encoder.
            if self.dimensions.is_some() || target != self.bitrate {
                self.inner = Self::build(target as u32, self.fps)?;
                self.bitrate = target;
            }
            self.want_idr = true;
        } else if target != self.bitrate {
            self.update_bitrate(target as u32)?;
            self.bitrate = target;
        }
        if self.want_idr {
            self.inner.force_intra_frame();
        }
        let (w, h) = (frame.width as usize, frame.height as usize);
        let stride = frame.stride as usize;
        let yuv = if stride.is_multiple_of(4) && frame.data.len() >= stride * h {
            match self.yuv_buf.take() {
                Some(mut buf) if buf.dimensions() == (w, h) => {
                    buf.read_bgra8(StridedBgra(frame));
                    buf
                }
                _ => YUVBuffer::from_bgra8_source(StridedBgra(frame)),
            }
        } else {
            bgra_to_i420_scalar(frame)
        };
        // OpenH264's encode() supplies Timestamp::ZERO. Real screen capture
        // is variable-rate: idle/CPU delays must replenish the bitrate budget
        // in elapsed time, rather than a fictitious fixed-FPS clock. Clamp
        // tightly repeated library calls to their configured FPS ceiling.
        let step = (1000.0 / self.fps.max(1.0)).floor().max(1.0) as u64;
        let stamp = now_ms
            .max(
                self.last_timestamp
                    .map_or(0, |last| last.saturating_add(step)),
            )
            .min(i64::MAX as u64);
        self.last_timestamp = Some(stamp);
        let stream = self
            .inner
            .encode_at(&yuv, openh264::Timestamp::from_millis(stamp))
            .map_err(|e| DesktopError::Encode(e.to_string()))?;
        self.yuv_buf = Some(yuv);
        self.dimensions = Some(dimensions);
        // The flag the writer's collapse trusts must be the truth on
        // the wire, not a schedule assumption: OpenH264 decides when
        // forced and periodic IDRs actually land, so read the NALs.
        let keyframe = bitstream_has_idr(&stream);
        // A rate-control skip must not consume a pending recovery request.
        // Clear only after an independent frame actually exists on the wire.
        if keyframe {
            self.want_idr = false;
        }
        Ok(EncodedFrame {
            codec: Codec::H264,
            data: Bytes::from(stream.to_vec()),
            keyframe,
        })
    }
}

impl H264Encoder {
    #[allow(unsafe_code)]
    fn update_bitrate(&mut self, bps: u32) -> Result<(), DesktopError> {
        use openh264_sys2::{
            ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SBitrateInfo, SPATIAL_LAYER_0,
            SPATIAL_LAYER_ALL,
        };
        let mut target = SBitrateInfo {
            iLayer: SPATIAL_LAYER_ALL,
            iBitrate: bps as i32,
        };
        let mut maximum = SBitrateInfo {
            iLayer: SPATIAL_LAYER_0,
            iBitrate: bps as i32,
        };
        let options = if u64::from(bps) > self.bitrate {
            [
                (ENCODER_OPTION_MAX_BITRATE, &mut maximum),
                (ENCODER_OPTION_BITRATE, &mut target),
            ]
        } else {
            [
                (ENCODER_OPTION_BITRATE, &mut target),
                (ENCODER_OPTION_MAX_BITRATE, &mut maximum),
            ]
        };
        for (option, value) in options {
            // SAFETY: encode_timed calls this only after successful initialization
            // with unchanged geometry, with exclusive &mut access. Each option
            // accepts exactly SBitrateInfo, consumed synchronously. Both rates
            // fit positive c_int; no layout, buffers or wrapper config change.
            let code = unsafe {
                self.inner
                    .raw_api()
                    .set_option(option, std::ptr::from_mut(value).cast())
            };
            if code != 0 {
                return Err(DesktopError::Encode(format!(
                    "live bitrate option {option} failed: {code}"
                )));
            }
        }
        Ok(())
    }
}

/// Whether the emitted bitstream carries an IDR picture — the flag the
/// writer's collapse trusts. Read off the NALs rather than assumed:
/// OpenH264 decides when forced and periodic IDRs actually land.
/// NAL units arrive Annex-B wrapped: a 3- or 4-byte start code, then
/// the header byte whose low five bits are the unit type (5 = IDR).
fn bitstream_has_idr(stream: &EncodedBitStream<'_>) -> bool {
    for l in 0..stream.num_layers() {
        let Some(layer) = stream.layer(l) else {
            continue;
        };
        for n in 0..layer.nal_count() {
            let Some(nal) = layer.nal_unit(n) else {
                continue;
            };
            // Annex-B start code: ≥2 leading zeros then 0x01; the byte
            // right after it is the NAL header. A NAL without that
            // prefix is not a start we can interpret.
            let Some(hdr) = nal
                .iter()
                .position(|&b| b != 0)
                .filter(|&i| i >= 2 && nal[i] == 1)
                .and_then(|i| nal.get(i + 1))
            else {
                continue;
            };
            if hdr & 0x1f == 5 {
                return true;
            }
        }
    }
    false
}

/// OpenH264 decoder producing RGB8 frames.
pub struct H264Decoder {
    inner: OhDecoder,
}

impl H264Decoder {
    pub fn new() -> Result<Self, DesktopError> {
        let api = OpenH264API::from_source();
        let inner = OhDecoder::with_api_config(api, DecoderConfig::default())
            .map_err(|e| DesktopError::Decode(e.to_string()))?;
        Ok(Self { inner })
    }
}

impl Decoder for H264Decoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<RawFrame>, DesktopError> {
        let Some(yuv) = self
            .inner
            .decode(&frame.data)
            .map_err(|e: OhError| DesktopError::Decode(e.to_string()))?
        else {
            return Ok(None);
        };
        let (w, h) = yuv.dimensions();
        // I420→RGBA in one SIMD pass (AVX2 on x86-64), then swap R↔B
        // in place for the RawFrame BGRA contract — the swap vectorizes
        // trivially and avoids a separate 3-byte-per-pixel scratch.
        let length = crate::frame_bytes(w, h).ok_or_else(|| {
            DesktopError::Decode("decoded dimensions exceed receive limit".into())
        })?;
        let mut bgra = vec![0u8; length];
        yuv.write_rgba8(&mut bgra);
        for px in bgra.as_chunks_mut::<4>().0.iter_mut() {
            px.swap(0, 2);
        }
        Ok(Some(RawFrame {
            width: w as u32,
            height: h as u32,
            stride: w as u32 * 4,
            data: Bytes::from(bgra),
        }))
    }
}

/// BGRA8 → I420 (BT.601 studio swing) for OpenH264, via the encoder
/// crate's own converter — it dispatches to AVX2 at runtime on x86-64
/// (scalar elsewhere), honors arbitrary row strides, and box-averages
/// chroma. Stride-incompatible buffers take the scalar path.
pub fn bgra_to_i420(frame: &RawFrame) -> YUVBuffer {
    let stride = frame.stride as usize;
    if stride.is_multiple_of(4) && frame.data.len() >= stride * frame.height as usize {
        return YUVBuffer::from_bgra8_source(StridedBgra(frame));
    }
    bgra_to_i420_scalar(frame)
}

/// `RawFrame` as an `openh264` BGRA source, carrying its real stride so
/// padded capture buffers need no intermediate copy.
struct StridedBgra<'a>(&'a RawFrame);

impl RGBSource for StridedBgra<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.0.width as usize, self.0.height as usize)
    }

    fn pixel_f32(&self, x: usize, y: usize) -> (f32, f32, f32) {
        let o = y * self.0.stride as usize + x * 4;
        let px = &self.0.data[o..o + 4];
        (px[2] as f32, px[1] as f32, px[0] as f32)
    }
}

impl RGB8Source for StridedBgra<'_> {
    fn dimensions_padded(&self) -> (usize, usize) {
        (self.0.stride as usize / 4, self.0.height as usize)
    }

    fn rgb8_data(&self) -> &[u8] {
        &self.0.data
    }

    fn pixel_stride(&self) -> usize {
        4
    }

    fn rgb_channel_offsets(&self) -> (usize, usize, usize) {
        (2, 1, 0)
    }
}

impl BGRA8Source for StridedBgra<'_> {}

/// Scalar fallback for strides that are not whole pixels — also the
/// test reference for the SIMD path's output.
fn bgra_to_i420_scalar(frame: &RawFrame) -> YUVBuffer {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let stride = frame.stride as usize;
    let mut yuv = vec![0u8; w * h * 3 / 2];
    let (y_plane, uv) = yuv.split_at_mut(w * h);
    let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
    let src = &frame.data;
    for row in 0..h {
        let srow = &src[row * stride..row * stride + w * 4];
        let yrow = &mut y_plane[row * w..row * w + w];
        for (x, px) in srow.as_chunks::<4>().0.iter().enumerate() {
            let (b, g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
            yrow[x] = clamp8((66 * r + 129 * g + 25 * b + 128) / 256 + 16);
            if row % 2 == 0 && x % 2 == 0 {
                let i = (row / 2) * (w / 2) + x / 2;
                u_plane[i] = clamp8((-38 * r - 74 * g + 112 * b + 128) / 256 + 128);
                v_plane[i] = clamp8((112 * r - 94 * g - 18 * b + 128) / 256 + 128);
            }
        }
    }
    YUVBuffer::from_vec(yuv, w, h)
}

fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> RawFrame {
        let (w, h) = (64u32, 64u32);
        RawFrame {
            width: w,
            height: h,
            stride: w * 4,
            data: Bytes::from(vec![0x5Au8; (w * h * 4) as usize]),
        }
    }

    /// The keyframe flag must describe the emitted bitstream: IDR on
    /// frame 0 (fresh encoder) and after `request_idr`, deltas between.
    #[test]
    fn keyframe_flag_matches_emitted_nals() {
        let mut enc = H264Encoder::new(1_000_000, 30.0).unwrap();
        assert!(enc.encode(&frame()).unwrap().keyframe, "first frame");
        for _ in 0..3 {
            assert!(
                !enc.encode(&frame()).unwrap().keyframe,
                "delta flagged as keyframe"
            );
        }
        enc.request_idr();
        assert!(
            enc.encode(&frame()).unwrap().keyframe,
            "forced IDR not flagged"
        );
        assert!(!enc.encode(&frame()).unwrap().keyframe);
    }

    /// Adaptation must preserve the exact decodable reference chain and apply
    /// both target/max native rates; merely changing our Rust bookkeeping is
    /// insufficient. Recreating an encoder here emits an IDR and fails this.
    #[test]
    #[allow(unsafe_code)]
    fn bitrate_changes_preserve_references_and_update_native_rates() {
        use openh264_sys2::{
            ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SBitrateInfo, SPATIAL_LAYER_0,
        };
        let mut enc = H264Encoder::new(1_000_000, 30.0).unwrap();
        let mut dec = H264Decoder::new().unwrap();
        dec.decode(&enc.encode_timed(&frame(), 0).unwrap())
            .unwrap()
            .unwrap();
        for (i, bps) in [1_100_000, 2_000_000, 100_000, 8_000_000, 500_000]
            .into_iter()
            .enumerate()
        {
            enc.set_bitrate(bps);
            let encoded = enc.encode_timed(&frame(), (i as u64 + 1) * 1_000).unwrap();
            assert!(
                !encoded.keyframe,
                "bitrate {bps} must not restart the reference chain"
            );
            assert!(dec.decode(&encoded).unwrap().is_some());
            for option in [ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE] {
                let mut actual = SBitrateInfo {
                    iLayer: SPATIAL_LAYER_0,
                    iBitrate: 0,
                };
                // SAFETY: initialized encoder, correctly typed synchronous
                // output pointer with exclusive access, no state mutation.
                let code = unsafe {
                    enc.inner
                        .raw_api()
                        .get_option(option, std::ptr::from_mut(&mut actual).cast())
                };
                assert_eq!(code, 0);
                assert_eq!(actual.iBitrate, bps as i32);
            }
        }
        enc.request_idr();
        assert!(enc.encode_timed(&frame(), 6_000).unwrap().keyframe);
    }

    #[test]
    fn variable_rate_capture_resumes_after_idle_without_clock_reset() {
        let mut encoder = H264Encoder::new(500_000, 60.0).unwrap();
        let first = encoder.encode_timed(&frame(), 0).unwrap();
        assert!(!first.data.is_empty());
        let mut pixels = vec![0u8; 64 * 64 * 4];
        for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            pixel.copy_from_slice(&[(i % 251) as u8, ((i / 64) * 3) as u8, 200, 255]);
        }
        let changed = RawFrame {
            width: 64,
            height: 64,
            stride: 256,
            data: Bytes::from(pixels),
        };
        let resumed = encoder.encode_timed(&changed, 5_000).unwrap();
        assert!(
            !resumed.data.is_empty(),
            "idle time must replenish the encoder budget"
        );
        let mut decoder = H264Decoder::new().unwrap();
        decoder.decode(&first).unwrap().unwrap();
        assert!(
            decoder.decode(&resumed).unwrap().is_some(),
            "idle resume must preserve the reference chain"
        );
        encoder.set_bitrate(1_000_000);
        let adapted = encoder.encode_timed(&changed, 6_000).unwrap();
        assert!(!adapted.keyframe);
        assert!(decoder.decode(&adapted).unwrap().is_some());
        assert_eq!(
            encoder.last_timestamp,
            Some(6_000),
            "rate adaptation must not reset its media clock"
        );
    }

    /// The SIMD/dispatched converter must agree with the scalar
    /// reference: identical luma (same BT.601 coefficients), chroma
    /// within box-average-vs-nearest tolerance.
    #[test]
    fn simd_conversion_matches_scalar() {
        let (w, h) = (64u32, 64u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let o = (y * w as usize + x) * 4;
                if x < 32 {
                    // Solid red half: box-average == nearest-sample.
                    data[o..o + 4].copy_from_slice(&[0, 0, 255, 255]);
                } else {
                    data[o] = (x % 256) as u8;
                    data[o + 1] = ((y * 3) % 256) as u8;
                    data[o + 2] = ((x * 2) % 256) as u8;
                    data[o + 3] = 255;
                }
            }
        }
        let raw = RawFrame {
            width: w,
            height: h,
            stride: w * 4,
            data: Bytes::from(data),
        };
        let fast = bgra_to_i420(&raw);
        let slow = bgra_to_i420_scalar(&raw);
        assert_eq!(fast.dimensions(), slow.dimensions());
        for (a, b) in fast.y().iter().zip(slow.y()) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 2, "Y diverges");
        }
        // Solid half: chroma must match tightly.
        for row in 0..h as usize / 2 {
            for x in 0..16 {
                let i = row * (w as usize / 2) + x;
                assert!(
                    (i32::from(fast.u()[i]) - i32::from(slow.u()[i])).abs() <= 2,
                    "U diverges on solid region"
                );
                assert!(
                    (i32::from(fast.v()[i]) - i32::from(slow.v()[i])).abs() <= 2,
                    "V diverges on solid region"
                );
            }
        }
        // Gradient half: same colorspace, different chroma sampling —
        // bounded divergence only.
        for (a, b) in fast.u().iter().zip(slow.u()) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 32, "U diverges");
        }
    }

    /// Padded strides must be honored — padding bytes never leak into
    /// the planes (filled with a sentinel).
    #[test]
    fn strided_input_ignores_padding() {
        let (w, h, pad) = (64u32, 64u32, 32u32);
        let tight = frame();
        let stride = (w * 4 + pad) as usize;
        let mut data = vec![0u8; stride * h as usize];
        for row in 0..h as usize {
            data[row * stride..row * stride + (w * 4) as usize]
                .copy_from_slice(&tight.data[row * w as usize * 4..(row + 1) * w as usize * 4]);
            for b in &mut data[row * stride + (w * 4) as usize..(row + 1) * stride] {
                *b = 0xAA;
            }
        }
        let padded = RawFrame {
            width: w,
            height: h,
            stride: stride as u32,
            data: Bytes::from(data),
        };
        let a = bgra_to_i420(&padded);
        let b = bgra_to_i420(&tight);
        assert_eq!(a.y(), b.y());
        assert_eq!(a.u(), b.u());
        assert_eq!(a.v(), b.v());
    }
}
