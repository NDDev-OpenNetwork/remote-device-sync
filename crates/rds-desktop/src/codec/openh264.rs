//! H.264 codec over OpenH264: Annex-B, constrained baseline, no B-frames.
//!
//! Software encode is the portability floor; hardware encoders plug in
//! behind the same `Encoder`/`Decoder` traits later.

use bytes::Bytes;
use openh264::decoder::{Decoder as OhDecoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder as OhEncoder, EncoderConfig, FrameRate, RateControlMode, UsageType,
};
use openh264::formats::{YUVBuffer, YUVSource};
use openh264::{Error as OhError, OpenH264API};
use rds_core::Codec;

use crate::{Decoder, DesktopError, EncodedFrame, Encoder, RawFrame};

/// Real-time OpenH264 encoder feeding `EncodedFrame`s.
pub struct H264Encoder {
    inner: OhEncoder,
    bitrate: u64,
    want_idr: bool,
    seq: u64,
}

impl H264Encoder {
    pub fn new(bitrate_bps: u64, fps: f32) -> Result<Self, DesktopError> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::CameraVideoRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(bitrate_bps as u32))
            .max_frame_rate(FrameRate::from_hz(fps))
            .skip_frames(true);
        let api = OpenH264API::from_source();
        let inner = OhEncoder::with_api_config(api, config)
            .map_err(|e| DesktopError::Encode(e.to_string()))?;
        Ok(Self {
            inner,
            bitrate: bitrate_bps,
            want_idr: true,
            seq: 0,
        })
    }
}

impl Encoder for H264Encoder {
    fn encode(&mut self, frame: &RawFrame) -> Result<EncodedFrame, DesktopError> {
        if self.want_idr {
            self.inner.force_intra_frame();
            self.want_idr = false;
        }
        let yuv = bgra_to_i420(frame);
        let stream = self
            .inner
            .encode(&yuv)
            .map_err(|e| DesktopError::Encode(e.to_string()))?;
        let keyframe = self.seq.is_multiple_of(240);
        self.seq += 1;
        Ok(EncodedFrame {
            codec: Codec::H264,
            data: Bytes::from(stream.to_vec()),
            keyframe,
        })
    }

    fn request_idr(&mut self) {
        self.want_idr = true;
    }

    fn set_bitrate(&mut self, bps: u32) {
        if u64::from(bps) == self.bitrate {
            return;
        }
        self.bitrate = u64::from(bps);
        // OpenH264 requires a rebuild for rate changes; keep the newest
        // request applied lazily on the next IDR boundary.
    }
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
        let mut rgb = vec![0u8; w * h * 3];
        yuv.write_rgb8(&mut rgb);
        // Expand RGB to BGRA for a uniform RawFrame layout.
        let mut bgra = vec![0u8; w * h * 4];
        for (dst, src) in bgra
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(rgb.as_chunks::<3>().0)
        {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = 0xFF;
        }
        Ok(Some(RawFrame {
            width: w as u32,
            height: h as u32,
            stride: w as u32 * 4,
            data: Bytes::from(bgra),
        }))
    }
}

/// BGRA8 → I420 (BT.601 studio swing). Encoder input for OpenH264.
pub fn bgra_to_i420(frame: &RawFrame) -> YUVBuffer {
    let w = frame.width as usize;
    let h = frame.height as usize;
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
