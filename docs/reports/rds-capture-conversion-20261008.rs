// Frozen paired benchmark input for rds-desktop-cpu-20261008.md.
use bytes::Bytes;
use openh264::formats::{BGRA8Source, RGB8Source, RGBSource, YUVBuffer, YUVSource};
use std::{hint::black_box, time::Instant};
#[derive(Clone, Copy)]
pub(crate) struct BgraFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data: &'a [u8],
}
struct StridedBgra<'a>(BgraFrame<'a>);

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
        self.0.data
    }

    fn pixel_stride(&self) -> usize {
        4
    }

    fn rgb_channel_offsets(&self) -> (usize, usize, usize) {
        (2, 1, 0)
    }
}

impl BGRA8Source for StridedBgra<'_> {}

fn main() {
    let source: Vec<u8> = (0..1920 * 1080 * 4).map(|i| (i % 251) as u8).collect();
    let rounds = 600usize;
    let raw = BgraFrame {
        width: 1920,
        height: 1080,
        stride: 7680,
        data: &source,
    };
    let mut yuv = YUVBuffer::from_bgra8_source(StridedBgra(raw));
    for pair in 0..5 {
        for copied in if pair % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let started = Instant::now();
            for _ in 0..rounds {
                if copied {
                    let owned = Bytes::copy_from_slice(black_box(&source));
                    let input = BgraFrame {
                        data: &owned,
                        ..raw
                    };
                    yuv.read_bgra8(StridedBgra(input));
                } else {
                    yuv.read_bgra8(StridedBgra(black_box(raw)));
                }
                black_box(yuv.y());
            }
            println!(
                "{{\"pair\":{pair},\"copied\":{copied},\"rounds\":{rounds},\"elapsed_ns\":{}}}",
                started.elapsed().as_nanos()
            );
        }
    }
}
