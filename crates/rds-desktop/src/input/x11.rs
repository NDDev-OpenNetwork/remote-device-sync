//! X11 input injection via XTEST (`x11rb`).
//!
//! Injects into the default X11 session. Portable baseline; Wayland paths
//! live in `input/portal` and `input/wlr`.

use std::collections::BTreeSet;

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
const MAX_SCROLL_CLICKS: f64 = 32.0;

/// XTEST input bound to one X screen, using the Linux XKB evdev keycode map.
/// Extended evdev keys outside core X11's 8-bit keycodes are refused.
pub struct XtestInput {
    conn: RustConnection,
    root: x11rb::protocol::xproto::Window,
    screen: u32,
    width: u16,
    height: u16,
    keycodes: std::ops::RangeInclusive<u8>,
    keys: BTreeSet<u8>,
    buttons: BTreeSet<u8>,
    scroll_x: f64,
    scroll_y: f64,
}

fn error(message: &str) -> DesktopError {
    DesktopError::Input(message.into())
}

impl XtestInput {
    /// The protocol's default display is X screen zero, independent of DISPLAY's suffix.
    pub fn new() -> Result<Self, DesktopError> {
        Self::for_display(0)
    }

    pub fn for_display(screen: u32) -> Result<Self, DesktopError> {
        let (conn, _) = RustConnection::connect(None).map_err(|e| error(&e.to_string()))?;
        conn.xtest_get_version(2, 2)
            .map_err(|e| error(&e.to_string()))?
            .reply()
            .map_err(|e| error(&e.to_string()))?;
        let setup = conn.setup();
        let display = setup
            .roots
            .get(screen as usize)
            .ok_or_else(|| error("X11 display does not exist"))?;
        let (root, width, height) = (
            display.root,
            display.width_in_pixels,
            display.height_in_pixels,
        );
        let keycodes = setup.min_keycode..=setup.max_keycode;
        Ok(Self {
            conn,
            root,
            screen,
            width,
            height,
            keycodes,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            scroll_x: 0.0,
            scroll_y: 0.0,
        })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), DesktopError> {
        // Check flushes and waits for server acceptance. A flushed but rejected
        // XTEST request must not produce a successful input acknowledgement.
        self.conn
            .xtest_fake_input(kind, detail, 0, self.root, x, y, 0)
            .map_err(|e| error(&e.to_string()))?
            .check()
            .map_err(|e| error(&e.to_string()))
    }

    fn pointer_on_screen(&self) -> Result<(), DesktopError> {
        if !self
            .conn
            .query_pointer(self.root)
            .map_err(|e| error(&e.to_string()))?
            .reply()
            .map_err(|e| error(&e.to_string()))?
            .same_screen
        {
            return Err(error("pointer input targets an inactive X11 screen"));
        }
        Ok(())
    }

    fn keyboard_on_screen(&self) -> Result<(), DesktopError> {
        let focus = self
            .conn
            .get_input_focus()
            .map_err(|e| error(&e.to_string()))?
            .reply()
            .map_err(|e| error(&e.to_string()))?
            .focus;
        if focus == u32::from(x11rb::protocol::xproto::InputFocus::POINTER_ROOT) {
            return self.pointer_on_screen();
        }
        if focus == x11rb::NONE {
            return Err(error("X11 keyboard has no focus"));
        }
        if self
            .conn
            .query_tree(focus)
            .map_err(|e| error(&e.to_string()))?
            .reply()
            .map_err(|e| error(&e.to_string()))?
            .root
            != self.root
        {
            return Err(error("keyboard focus is on another X11 screen"));
        }
        Ok(())
    }

    fn key(&mut self, code: u32, pressed: bool) -> Result<(), DesktopError> {
        // Xorg's evdev map reserves the first eight X keycodes.
        let key = code
            .checked_add(8)
            .and_then(|key| u8::try_from(key).ok())
            .filter(|key| code != 0 && self.keycodes.contains(key))
            .ok_or_else(|| error("evdev key is outside the X11 keycode range"))?;
        if pressed {
            self.keyboard_on_screen()?;
        } else if !self.keys.contains(&key) {
            return Ok(());
        }
        self.fake(if pressed { KEY_PRESS } else { KEY_RELEASE }, key, 0, 0)?;
        if pressed {
            self.keys.insert(key);
        } else {
            self.keys.remove(&key);
        }
        Ok(())
    }

    fn button(&mut self, button: u8, pressed: bool) -> Result<(), DesktopError> {
        if pressed {
            self.pointer_on_screen()?;
        } else if !self.buttons.contains(&button) {
            return Ok(());
        }
        self.fake(
            if pressed {
                BUTTON_PRESS
            } else {
                BUTTON_RELEASE
            },
            button,
            0,
            0,
        )?;
        if pressed {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        Ok(())
    }

    fn scroll(&mut self, dx: f64, dy: f64) -> Result<(), DesktopError> {
        if !dx.is_finite() || !dy.is_finite() {
            return Err(error("scroll deltas must be finite"));
        }
        self.pointer_on_screen()?;
        // Preserve sub-step input. Bound work per event, including accumulated
        // fractions, without allowing NaN to become a page of wheel clicks.
        self.scroll_x = (self.scroll_x + dx).clamp(-MAX_SCROLL_CLICKS, MAX_SCROLL_CLICKS);
        self.scroll_y = (self.scroll_y + dy).clamp(-MAX_SCROLL_CLICKS, MAX_SCROLL_CLICKS);
        let x = self.scroll_x.trunc() as i32;
        let y = self.scroll_y.trunc() as i32;
        self.scroll_x -= f64::from(x);
        self.scroll_y -= f64::from(y);
        // Protocol convention: positive y is up, positive x is left.
        for (steps, negative, positive) in [(y, 5, 4), (x, 7, 6)] {
            let button = if steps > 0 { positive } else { negative };
            for _ in 0..steps.unsigned_abs() {
                self.button(button, true)?;
                self.button(button, false)?;
            }
        }
        Ok(())
    }
}

fn coordinate(value: f64, extent: u16) -> Result<i16, DesktopError> {
    if !value.is_finite()
        || value < 0.0
        || value >= f64::from(extent)
        || value > f64::from(i16::MAX)
    {
        return Err(error("pointer position is outside the X11 display"));
    }
    Ok(value.floor() as i16)
}

fn delta(value: f64) -> Result<i16, DesktopError> {
    if !value.is_finite()
        || value.round() < f64::from(i16::MIN)
        || value.round() > f64::from(i16::MAX)
    {
        return Err(error("relative pointer delta is outside XTEST range"));
    }
    Ok(value.round() as i16)
}

impl InputSink for XtestInput {
    fn inject(&mut self, event: &InputEvent) -> Result<(), DesktopError> {
        if event.display_id != self.screen {
            return Err(error("input event targets another X11 display"));
        }
        match event.kind {
            InputKind::KeyDown { code } => self.key(code, true),
            InputKind::KeyUp { code } => self.key(code, false),
            InputKind::PointerMove { x, y } => self
                .conn
                .warp_pointer(
                    x11rb::NONE,
                    self.root,
                    0,
                    0,
                    0,
                    0,
                    coordinate(x, self.width)?,
                    coordinate(y, self.height)?,
                )
                .map_err(|e| error(&e.to_string()))?
                .check()
                .map_err(|e| error(&e.to_string())),
            InputKind::PointerMotion { dx, dy } => {
                let (dx, dy) = (delta(dx)?, delta(dy)?);
                self.pointer_on_screen()?;
                // XTEST detail=1 is relative motion; WarpPointer with a root
                // destination uses absolute coordinates instead.
                self.fake(MOTION_NOTIFY, 1, dx, dy)
            }
            InputKind::PointerButton { button, pressed } => {
                let button = match button {
                    0x110 => 1, // BTN_LEFT
                    0x111 => 3, // BTN_RIGHT
                    0x112 => 2, // BTN_MIDDLE
                    0x113..=0x117 => (button - 0x113 + 8) as u8,
                    _ => return Err(error("unsupported evdev pointer button")),
                };
                self.button(button, pressed)
            }
            InputKind::Scroll { dx, dy } => self.scroll(dx, dy),
        }
    }
}

impl Drop for XtestInput {
    fn drop(&mut self) {
        // Best-effort release of only this sink's injected holds. The worker
        // drops the sink after its last in-flight call, outside Tokio workers.
        for key in &self.keys {
            let _ = self
                .conn
                .xtest_fake_input(KEY_RELEASE, *key, 0, self.root, 0, 0, 0);
        }
        for button in &self.buttons {
            let _ = self
                .conn
                .xtest_fake_input(BUTTON_RELEASE, *button, 0, self.root, 0, 0, 0);
        }
        let _ = self.conn.flush();
        // Keep the X connection alive until release requests have reached the
        // server. Closing immediately after flush can discard queued input.
        if let Ok(cookie) = self.conn.get_input_focus() {
            let _ = cookie.reply();
        }
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
