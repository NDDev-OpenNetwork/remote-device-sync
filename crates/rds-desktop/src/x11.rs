//! X11 capture and input via `x11rb`: `GetImage` polling for frames and
//! XTEST for input injection.
//!
//! This is the portable baseline backend. The MIT-SHM zero-copy path and
//! the Wayland portal+PipeWire backend are scheduled behind the same
//! traits; GetImage polling is what every X server already supports.

use bytes::Bytes;
use rds_core::{DesktopCaps, DisplayInfo, InputEvent};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt, GetImageReply, ImageFormat};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::{Capturer, DesktopError, InputSink, RawFrame};

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

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

/// XTEST input sink: injects events into the default X11 session.
pub struct XtestInput {
    conn: RustConnection,
    root: x11rb::protocol::xproto::Window,
}

impl XtestInput {
    pub fn new() -> Result<Self, DesktopError> {
        let (conn, screen) =
            RustConnection::connect(None).map_err(|e| DesktopError::Input(e.to_string()))?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root })
    }
}

impl InputSink for XtestInput {
    fn inject(&mut self, event: &InputEvent) -> Result<(), DesktopError> {
        match *event {
            InputEvent::KeyDown { code } => self
                .conn
                .xtest_fake_input(KEY_PRESS, code as u8, 0, self.root, 0, 0, 0),
            InputEvent::KeyUp { code } => {
                self.conn
                    .xtest_fake_input(KEY_RELEASE, code as u8, 0, self.root, 0, 0, 0)
            }
            InputEvent::PointerMove { x, y } => {
                self.conn
                    .xtest_fake_input(MOTION_NOTIFY, 0, 0, self.root, x as i16, y as i16, 0)
            }
            InputEvent::PointerMotion { dx, dy } => {
                // Relative motion needs current position; XTEST lacks a
                // relative primitive, so use XWarpPointer instead. The
                // cookie is discarded; the flush below delivers it.
                let _ = self
                    .conn
                    .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, dx as i16, dy as i16)
                    .map_err(|e| DesktopError::Input(e.to_string()))?;
                return self
                    .conn
                    .flush()
                    .map_err(|e| DesktopError::Input(e.to_string()));
            }
            InputEvent::PointerButton { button, pressed } => self.conn.xtest_fake_input(
                if pressed {
                    BUTTON_PRESS
                } else {
                    BUTTON_RELEASE
                },
                button as u8,
                0,
                self.root,
                0,
                0,
                0,
            ),
            InputEvent::Scroll { dx, dy } => {
                // Emulate wheel clicks: 4 up, 5 down, 6 left, 7 right.
                for _ in 0..dy.abs().round() as u32 {
                    let b = if dy > 0.0 { 4 } else { 5 };
                    self.conn
                        .xtest_fake_input(BUTTON_PRESS, b, 0, self.root, 0, 0, 0)
                        .map_err(|e| DesktopError::Input(e.to_string()))?;
                    self.conn
                        .xtest_fake_input(BUTTON_RELEASE, b, 0, self.root, 0, 0, 0)
                        .map_err(|e| DesktopError::Input(e.to_string()))?;
                }
                for _ in 0..dx.abs().round() as u32 {
                    let b = if dx > 0.0 { 6 } else { 7 };
                    self.conn
                        .xtest_fake_input(BUTTON_PRESS, b, 0, self.root, 0, 0, 0)
                        .map_err(|e| DesktopError::Input(e.to_string()))?;
                    self.conn
                        .xtest_fake_input(BUTTON_RELEASE, b, 0, self.root, 0, 0, 0)
                        .map_err(|e| DesktopError::Input(e.to_string()))?;
                }
                return Ok(());
            }
        }
        .map_err(|e| DesktopError::Input(e.to_string()))?;
        self.conn
            .flush()
            .map_err(|e| DesktopError::Input(e.to_string()))
    }
}

/// Convenience one-shot injection used by the session control loop.
pub fn inject(event: &InputEvent) -> Result<(), DesktopError> {
    use std::sync::Mutex;
    static SINK: Mutex<Option<XtestInput>> = Mutex::new(None);
    let mut guard = SINK
        .lock()
        .map_err(|_| DesktopError::Input("input sink poisoned".into()))?;
    if guard.is_none() {
        *guard = Some(XtestInput::new()?);
    }
    guard.as_mut().unwrap().inject(event)
}
