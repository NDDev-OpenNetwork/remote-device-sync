//! X11 capture via `x11rb`: MIT-SHM shared-memory `GetImage` where the
//! server supports it, plain `GetImage` polling as the portable
//! fallback.
//!
//! MIT-SHM removes the dominant capture cost: the pixmap is written
//! into a shared segment the client mmaps, so a 1080p frame costs one
//! completion event plus a memcpy instead of ~8 MiB serialized through
//! the X socket. `CreateSegment` (SHM ≥ 1.2 fd passing) is preferred —
//! the server allocates and owns nothing persists on the client side.
//! `ext-image-copy-capture` and the DRM/KMS tap remain scheduled behind
//! the same `Capturer` trait. Input injection lives in
//! `crate::input::x11`.
//!
//! `mmap` on the server-provided segment is the one unsafe call in this
//! module (workspace convention: FFI paths allow `unsafe_code` locally).
#![allow(unsafe_code)]

use std::os::fd::OwnedFd;

use bytes::Bytes;
use memmap2::{MmapMut, MmapOptions};
use rds_core::{DesktopCaps, DisplayInfo};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as _, Damage, ReportLevel};
use x11rb::protocol::shm::{self, ConnectionExt as _, Seg};
use x11rb::protocol::xproto::{ConnectionExt, GetImageReply, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::{Capturer, DesktopError, RawFrame};

/// MIT-SHM segment the server fills in place — the reply is a
/// completion event, not a serialized pixmap.
struct ShmPath {
    seg: Seg,
    map: MmapMut,
    len: usize,
    /// The mapping stays valid without it, but keeping the fd makes
    /// ownership/lifetime explicit.
    _fd: OwnedFd,
}

/// Polls one X11 screen (root window) into `RawFrame`s.
pub struct X11Capturer {
    conn: RustConnection,
    root: x11rb::protocol::xproto::Window,
    width: u16,
    height: u16,
    screen: usize,
    shm: Option<ShmPath>,
    /// DAMAGE object on the root window — idle detection so a still
    /// desktop costs no capture/encode work at all.
    damage: Option<Damage>,
    /// Sticky flag: the first `changed` call must be true (the screen
    /// existed before the damage object did).
    dirty: bool,
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
        let shm = Self::try_shm(&conn, width, height);
        let damage = Self::try_damage(&conn, root);
        Ok(Self {
            conn,
            root,
            width,
            height,
            screen: idx,
            shm,
            damage,
            dirty: true,
        })
    }

    /// MIT-SHM ≥ 1.2 (fd passing): the server creates the segment and
    /// hands back an fd we mmap. `None` on older servers, remote
    /// displays, or any setup failure — the caller falls back to plain
    /// `GetImage`.
    fn try_shm(conn: &RustConnection, width: u16, height: u16) -> Option<ShmPath> {
        conn.extension_information(shm::X11_EXTENSION_NAME).ok()??;
        let ver = conn.shm_query_version().ok()?.reply().ok()?;
        if (ver.major_version, ver.minor_version) < (1, 2) {
            return None;
        }
        let len = usize::from(width) * usize::from(height) * 4;
        let seg = conn.generate_id().ok()?;
        let reply = conn
            .shm_create_segment(seg, len as u32, false)
            .ok()?
            .reply()
            .ok()?;
        let fd: OwnedFd = reply.shm_fd;
        // SAFETY: `fd` is a fresh server-created segment of exactly
        // `len` bytes; nothing else mutates its size.
        let map = unsafe { MmapOptions::new().len(len).map_mut(&fd) }.ok()?;
        Some(ShmPath {
            seg,
            map,
            len,
            _fd: fd,
        })
    }

    /// DAMAGE (XFixes ≥ 4): one object on the root window reporting
    /// `NON_EMPTY` transitions — enough for a changed/not-changed bit.
    /// `None` where the extension is absent.
    fn try_damage(conn: &RustConnection, root: x11rb::protocol::xproto::Window) -> Option<Damage> {
        conn.extension_information(damage::X11_EXTENSION_NAME)
            .ok()??;
        conn.damage_query_version(1, 1).ok()?.reply().ok()?;
        let dmg = conn.generate_id().ok()?;
        conn.damage_create(dmg, root, ReportLevel::NON_EMPTY)
            .ok()?
            .check()
            .ok()?;
        Some(dmg)
    }
}

impl Capturer for X11Capturer {
    fn capture(&mut self) -> Result<RawFrame, DesktopError> {
        if let Some(shm) = &self.shm {
            // `reply()` waits for the ShmCompletion event the server
            // sends after filling the segment.
            let filled = self
                .conn
                .shm_get_image(
                    self.root,
                    0,
                    0,
                    self.width,
                    self.height,
                    !0,
                    ImageFormat::Z_PIXMAP.into(),
                    shm.seg,
                    0,
                )
                .map_err(|e| DesktopError::Capture(e.to_string()))
                .and_then(|c| c.reply().map_err(|e| DesktopError::Capture(e.to_string())));
            match filled {
                Ok(_) => {
                    return Ok(RawFrame {
                        width: u32::from(self.width),
                        height: u32::from(self.height),
                        stride: u32::from(self.width) * 4,
                        data: Bytes::copy_from_slice(&shm.map[..shm.len]),
                    });
                }
                Err(e) => {
                    tracing::warn!("MIT-SHM capture failed ({e}); falling back to GetImage");
                }
            }
        }
        // Reached only when the shm attempt failed at runtime — stop
        // paying for attempts that can no longer succeed.
        self.shm = None;
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

    /// Damage-driven change detection: drains pending events; any
    /// `DamageNotify` on our object marks the screen dirty. `NON_EMPTY`
    /// reports only the empty→non-empty transition, so a dirty read
    /// subtracts the region back to empty to re-arm notification.
    fn changed(&mut self) -> bool {
        let mut dirty = std::mem::take(&mut self.dirty);
        let Some(dmg) = self.damage else {
            return true;
        };
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            if matches!(ev, Event::DamageNotify(e) if e.damage == dmg) {
                dirty = true;
            }
        }
        if dirty {
            let _ = self.conn.damage_subtract(dmg, 0u32, 0u32);
        }
        dirty
    }
}

impl Drop for X11Capturer {
    fn drop(&mut self) {
        if let Some(dmg) = self.damage.take() {
            let _ = self.conn.damage_destroy(dmg);
        }
        if let Some(shm) = self.shm.take() {
            let _ = self.conn.shm_detach(shm.seg);
        }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Real capture needs an X server — skipped silently when `$DISPLAY`
    /// is unset (CI has none). On a local server (`:N`) MIT-SHM must be
    /// present; remote `host:N` displays legitimately lack it.
    #[test]
    fn capture_roundtrip() {
        let Some(display) = std::env::var_os("DISPLAY") else {
            return;
        };
        let local = display.to_string_lossy().starts_with(':');
        let Ok(mut cap) = X11Capturer::new(0) else {
            return;
        };
        if local {
            assert!(cap.shm.is_some(), "local X server without MIT-SHM 1.2?");
            assert!(cap.damage.is_some(), "local X server without DAMAGE?");
        }
        for _ in 0..3 {
            let frame = cap.capture().unwrap();
            assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);
            assert_eq!(frame.stride, frame.width * 4);
        }
        if !local {
            return;
        }
        // Damage bookkeeping: first read is dirty (screen predates the
        // damage object), a still screen then reports clean, and a
        // server-side repaint re-dirties it.
        assert!(cap.changed(), "first changed() must be dirty");
        assert!(!cap.changed(), "still screen reported damage");
        x11rb::protocol::xproto::clear_area(&cap.conn, false, cap.root, 0, 0, 100, 100).unwrap();
        cap.conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(cap.changed(), "root repaint produced no damage");
    }
}
