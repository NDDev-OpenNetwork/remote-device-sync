//! ScreenCast pixels map into the matching EIS region, not portal positions.

use crate::DesktopError;

#[derive(Clone, Debug)]
pub(super) struct Region {
    pub mapping_id: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub fn map(
        &self,
        mapping_id: &str,
        pixels: (u32, u32),
        point: (f64, f64),
    ) -> Result<(f64, f64), DesktopError> {
        let (width, height) = pixels;
        let (x, y) = point;
        if self.mapping_id != mapping_id
            || self.mapping_id.is_empty()
            || width == 0
            || height == 0
            || self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).is_none()
            || self.y.checked_add(self.height).is_none()
            || !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x >= f64::from(width)
            || y >= f64::from(height)
        {
            return Err(DesktopError::Input(
                "invalid portal display coordinates".into(),
            ));
        }
        // The EIS region already includes compositor scale and coordinate
        // normalization. Multiplying by physical_scale again is incorrect.
        Ok((
            f64::from(self.x) + x * f64::from(self.width) / f64::from(width),
            f64::from(self.y) + y * f64::from(self.height) / f64::from(height),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_monitors_use_their_own_fractional_scale_and_offset() {
        let high = Region {
            mapping_id: "high".into(),
            x: 1920,
            y: 0,
            width: 2880,
            height: 1620,
        };
        let low = Region {
            mapping_id: "low".into(),
            x: 0,
            y: 540,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            high.map("high", (3840, 2160), (1920.0, 1080.0)).unwrap(),
            (3360.0, 810.0)
        );
        assert_eq!(
            low.map("low", (1920, 1080), (960.0, 540.0)).unwrap(),
            (960.0, 1080.0)
        );
        assert!(high.map("low", (3840, 2160), (0.0, 0.0)).is_err());
    }

    #[test]
    fn invalid_points_and_extents_cannot_escape_the_selected_region() {
        let region = Region {
            mapping_id: "display".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        for point in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (-1.0, 0.0),
            (1920.0, 0.0),
            (0.0, 1080.0),
        ] {
            assert!(region.map("display", (1920, 1080), point).is_err());
        }
        assert!(region.map("display", (0, 1080), (0.0, 0.0)).is_err());
        let invalid = Region {
            x: u32::MAX,
            ..region
        };
        assert!(invalid.map("display", (1920, 1080), (0.0, 0.0)).is_err());
    }
}
