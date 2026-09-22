//! X11 input injection via XTEST (`x11rb`).
//!
//! Injects into the default X11 session. Portable baseline; Wayland paths
//! live in `input/portal` and `input/wlr`.

use rds_core::{InputEvent, InputKind};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::{DesktopError, InputSink};

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

/// Max wheel clicks injected per scroll event. Remote-supplied deltas
/// are unbounded f64s — without a cap a single event can loop billions
/// of paired XTEST calls and wedge the injector permanently. 32 lines
/// is already a page-scale scroll.
const MAX_SCROLL_CLICKS: f64 = 32.0;

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
        match event.kind {
            InputKind::KeyDown { code } => self
                .conn
                .xtest_fake_input(KEY_PRESS, code as u8, 0, self.root, 0, 0, 0),
            InputKind::KeyUp { code } => {
                self.conn
                    .xtest_fake_input(KEY_RELEASE, code as u8, 0, self.root, 0, 0, 0)
            }
            InputKind::PointerMove { x, y } => {
                self.conn
                    .xtest_fake_input(MOTION_NOTIFY, 0, 0, self.root, x as i16, y as i16, 0)
            }
            InputKind::PointerMotion { dx, dy } => {
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
            InputKind::PointerButton { button, pressed } => self.conn.xtest_fake_input(
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
            InputKind::Scroll { dx, dy } => {
                // Emulate wheel clicks: 4 up, 5 down, 6 left, 7 right.
                // `.min` bounds the loop; NaN mins to NaN → 0 clicks.
                for _ in 0..dy.abs().min(MAX_SCROLL_CLICKS).round() as u32 {
                    let b = if dy > 0.0 { 4 } else { 5 };
                    self.conn
                        .xtest_fake_input(BUTTON_PRESS, b, 0, self.root, 0, 0, 0)
                        .map_err(|e| DesktopError::Input(e.to_string()))?;
                    self.conn
                        .xtest_fake_input(BUTTON_RELEASE, b, 0, self.root, 0, 0, 0)
                        .map_err(|e| DesktopError::Input(e.to_string()))?;
                }
                for _ in 0..dx.abs().min(MAX_SCROLL_CLICKS).round() as u32 {
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
