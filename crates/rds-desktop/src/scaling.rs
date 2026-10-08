//! Owned low-latency BGRA downscaling for a configured serving profile. The
//! display capabilities keep original coordinates; only video dimensions change.
use crate::{BgraFrame, DesktopError, RawFrame};
use bytes::Bytes;

pub(crate) fn downscale(frame: BgraFrame<'_>, height: u32) -> Result<RawFrame, DesktopError> {
    let height = height & !1;
    let width =
        ((u64::from(frame.width) * u64::from(height) / u64::from(frame.height)) as u32) & !1;
    let length = crate::frame_bytes(width as usize, height as usize)
        .ok_or_else(|| DesktopError::Encode("invalid scaled dimensions".into()))?;
    let source_row = frame
        .width
        .checked_mul(4)
        .ok_or_else(|| DesktopError::Encode("invalid source width".into()))?;
    let source_len = (frame.stride as usize)
        .checked_mul(frame.height as usize)
        .ok_or_else(|| DesktopError::Encode("invalid source stride".into()))?;
    if frame.stride < source_row || frame.data.len() < source_len {
        return Err(DesktopError::Encode("invalid source BGRA layout".into()));
    }
    let mut out = vec![0u8; length];
    for y in 0..height {
        let sy = u64::from(y) * u64::from(frame.height) / u64::from(height);
        let source = sy as usize * frame.stride as usize;
        let target = y as usize * width as usize * 4;
        for x in 0..width {
            let sx = u64::from(x) * u64::from(frame.width) / u64::from(width);
            let offset = source + sx as usize * 4;
            let destination = target + x as usize * 4;
            out[destination..destination + 4].copy_from_slice(&frame.data[offset..offset + 4]);
        }
    }
    Ok(RawFrame {
        width,
        height,
        stride: width * 4,
        data: Bytes::from(out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downscaling_preserves_aspect_color_and_ignores_row_padding() {
        let mut bytes = Vec::new();
        for _ in 0..8 {
            bytes.extend_from_slice(&[vec![12u8; 16 * 4], vec![99; 8]].concat());
        }
        let frame = RawFrame {
            width: 16,
            height: 8,
            stride: 72,
            data: Bytes::from(bytes),
        };
        let resized = downscale((&frame).into(), 4).unwrap();
        assert_eq!((resized.width, resized.height, resized.stride), (8, 4, 32));
        assert_eq!(&resized.data[..], &vec![12; 128]);
    }
}
