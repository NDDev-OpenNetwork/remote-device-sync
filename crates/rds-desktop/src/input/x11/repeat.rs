//! Client-paced repeats with real native holds, shared by keyboard controllers.
use crate::DesktopError;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use x11rb::protocol::xkb::{ConnectionExt as _, Control, ID};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

struct State {
    owners: u32,
    repeat: bool,
}
static HOLDS: Mutex<BTreeMap<(String, u8), State>> = Mutex::new(BTreeMap::new());

pub(super) struct Hold {
    conn: Arc<RustConnection>,
    root: u32,
    server: String,
    pub(super) key: u8,
    released: bool,
}

fn error() -> DesktopError {
    DesktopError::Input("X11 keyboard repeat state failed".into())
}
// Core ChangeKeyboardControl marks the key's repeat setting explicit and emits
// XkbMapNotify even for a no-op. Compositors may rebuild the whole keymap for
// every press/release. XKB per-key controls change repeat without map changes.
// Callers hold HOLDS while reading/modifying the array, so our controllers cannot
// overwrite each other's suppression. All other controls/array bits are retained.
fn mode(conn: &RustConnection, key: u8, on: bool) -> Result<bool, DesktopError> {
    let keyboard = conn
        .xkb_get_controls(ID::USE_CORE_KBD.into())
        .map_err(|_| error())?
        .reply()
        .map_err(|_| error())?;
    let mut repeats = keyboard.per_key_repeat;
    let byte = usize::from(key) / 8;
    let mask = 1 << (key % 8);
    let previous = repeats[byte] & mask != 0;
    if previous == on {
        return Ok(previous);
    }
    if on {
        repeats[byte] |= mask;
    } else {
        repeats[byte] &= !mask;
    }
    conn.xkb_set_controls(
        ID::USE_CORE_KBD.into(),
        0u16.into(),
        0u16.into(),
        0u16.into(),
        0u16.into(),
        0u16.into(),
        0u16.into(),
        0u16.into(),
        0u16.into(),
        keyboard.mouse_keys_dflt_btn,
        keyboard.groups_wrap,
        keyboard.access_x_option,
        0u32.into(),
        0u32.into(),
        Control::PER_KEY_REPEAT,
        keyboard.repeat_delay,
        keyboard.repeat_interval,
        keyboard.slow_keys_delay,
        keyboard.debounce_delay,
        keyboard.mouse_keys_delay,
        keyboard.mouse_keys_interval,
        keyboard.mouse_keys_time_to_max,
        keyboard.mouse_keys_max_speed,
        keyboard.mouse_keys_curve,
        keyboard.access_x_timeout,
        keyboard.access_x_timeout_mask,
        keyboard.access_x_timeout_values,
        keyboard.access_x_timeout_options_mask,
        keyboard.access_x_timeout_options_values,
        &repeats,
    )
    .map_err(|_| error())?
    .check()
    .map_err(|_| error())?;
    Ok(previous)
}
fn fake(conn: &RustConnection, root: u32, key: u8, press: bool) -> Result<(), DesktopError> {
    conn.xtest_fake_input(if press { 2 } else { 3 }, key, 0, root, 0, 0, 0)
        .map_err(|_| error())?
        .check()
        .map_err(|_| error())
}

impl Hold {
    pub(super) fn press(
        conn: Arc<RustConnection>,
        root: u32,
        server: &str,
        key: u8,
        modifier: bool,
    ) -> Result<Self, DesktopError> {
        let mut holds = HOLDS.lock().map_err(|_| error())?;
        let id = (server.to_owned(), key);
        if let Some(state) = holds.get_mut(&id) {
            let next = state.owners.checked_add(1).ok_or_else(error)?;
            if !modifier {
                fake(&conn, root, key, false)?;
                fake(&conn, root, key, true)?;
            }
            state.owners = next;
        } else {
            let repeat = mode(&conn, key, false)?;
            if let Err(e) = fake(&conn, root, key, true) {
                let _ = mode(&conn, key, repeat);
                return Err(e);
            }
            holds.insert(id, State { owners: 1, repeat });
        }
        Ok(Self {
            conn,
            root,
            server: server.to_owned(),
            key,
            released: false,
        })
    }

    pub(super) fn repeat(&self, modifier: bool) -> Result<(), DesktopError> {
        if modifier {
            return Ok(());
        }
        // A repeated client KeyDown is an intentional native repeat. Core X
        // suppresses duplicate presses with typematic disabled, so pulse once.
        let _holds = HOLDS.lock().map_err(|_| error())?;
        fake(&self.conn, self.root, self.key, false)?;
        fake(&self.conn, self.root, self.key, true)
    }

    pub(super) fn release(&mut self) -> Result<(), DesktopError> {
        if self.released {
            return Ok(());
        }
        let mut holds = HOLDS.lock().map_err(|_| error())?;
        let id = (self.server.clone(), self.key);
        let Some(state) = holds.get_mut(&id) else {
            self.released = true;
            return Ok(());
        };
        if state.owners > 1 {
            state.owners -= 1;
            self.released = true;
            return Ok(());
        }
        let release = fake(&self.conn, self.root, self.key, false);
        let restore = mode(&self.conn, self.key, state.repeat);
        holds.remove(&id);
        self.released = true;
        release.and(restore.map(|_| ()))
    }
}
impl Drop for Hold {
    fn drop(&mut self) {
        if self.release().is_err() {
            tracing::warn!("X11 keyboard hold cleanup failed");
        }
    }
}
