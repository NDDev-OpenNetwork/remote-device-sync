//! X11 capture via `x11rb`: `GetImage` polling for frames.
//!
//! This is the portable baseline backend. The MIT-SHM zero-copy path,
//! `ext-image-copy-capture` and the DRM/KMS tap are scheduled behind the
//! same `Capturer` trait; GetImage polling is what every X server already
//! supports. Input injection lives in `crate::input::x11`.

use bytes::Bytes;
use rds_core::{DesktopCaps, DisplayInfo};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt, GetImageReply, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::{Capturer, DesktopError, RawFrame};

/// Polls one X11 screen (root window) into `RawFrame`s.
pub struct X11Capturer {
    conn: RustConnection,
    root: x11rb::protocol::xproto::Window,
    width: u16,
    height: u16,
    screen: usize,
}

impl X11Capturer {
    /// Connect to `$DISPLAY` and select `screen`.
    pub fn new(screen: u32) -> Result<Self, DesktopError> {
        let (conn, default_screen) =
            RustConnection::connect(None).map_err(|e| DesktopError::Capture(e.to_string()))?;
        let setup = conn.setup();
        let idx = (screen as usize).min(setup.roots.len().saturating_sub(1));
        let root = setup.roots[idx].root;
        let (width, height) = (
            setup.roots[idx].width_in_pixels,
            setup.roots[idx].height_in_pixels,
        );
        let _ = default_screen;
        Ok(Self {
            conn,
            root,
            width,
            height,
            screen: idx,
        })
    }
}

impl Capturer for X11Capturer {
    fn capture(&mut self) -> Result<RawFrame, DesktopError> {
        let reply: GetImageReply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                0,
                0,
                self.width,
                self.height,
                !0,
            )
            .map_err(|e| DesktopError::Capture(e.to_string()))?
            .reply()
            .map_err(|e| DesktopError::Capture(e.to_string()))?;
        Ok(RawFrame {
            width: u32::from(self.width),
            height: u32::from(self.height),
            stride: u32::from(self.width) * 4,
            data: Bytes::from(reply.data),
        })
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        vec![DisplayInfo {
            index: self.screen as u32,
            width: u32::from(self.width),
            height: u32::from(self.height),
            primary: true,
        }]
    }
}

/// Displays visible over X11.
pub fn capabilities() -> Result<DesktopCaps, DesktopError> {
    match X11Capturer::new(0) {
        Ok(c) => Ok(DesktopCaps {
            displays: c.displays(),
            codecs: vec![rds_core::Codec::H264],
        }),
        Err(e) => Err(e),
    }
}
