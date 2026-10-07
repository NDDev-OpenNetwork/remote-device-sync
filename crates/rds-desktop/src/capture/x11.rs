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
    /// One owned frame buffer returned by the producer after encoding. The
    /// next capture fills it in place, avoiding allocator churn at 60 fps.
    recycled: Option<Bytes>,
}

impl X11Capturer {
    /// Wait for X socket readiness instead of periodically polling damage.
    /// The bounded timeout also lets IDR and teardown be observed promptly.
    pub fn wait_for_change(&mut self, timeout: std::time::Duration) -> bool {
        if self.changed() {
            return true;
        }
        let mut fds = [rustix::event::PollFd::new(
            self.conn.stream(),
            rustix::event::PollFlags::IN,
        )];
        let timeout = rustix::event::Timespec {
            tv_sec: timeout.as_secs() as i64,
            tv_nsec: i64::from(timeout.subsec_nanos()),
        };
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(_)
                if fds[0].revents().intersects(
                    rustix::event::PollFlags::ERR
                        | rustix::event::PollFlags::HUP
                        | rustix::event::PollFlags::NVAL,
                ) =>
            {
                true
            }
            Ok(_) => self.changed(),
            Err(_) => true,
        }
    }
    /// Connect to `$DISPLAY` and select `screen`.
    pub fn new(screen: u32) -> Result<Self, DesktopError> {
        let (conn, default_screen) =
            RustConnection::connect(None).map_err(|e| DesktopError::Capture(e.to_string()))?;
        let setup = conn.setup();
        let idx = screen as usize;
        let display = setup
            .roots
            .get(idx)
            .ok_or_else(|| DesktopError::Capture("X11 display does not exist".into()))?;
        let root = display.root;
        let (width, height) = (display.width_in_pixels, display.height_in_pixels);
        // Frames are decoded as packed 32bpp pixels; a server whose root
        // depth has no 32bpp pixmap format cannot produce them — refuse
        // honestly rather than panic or emit corrupt frames.
        let ok = setup
            .pixmap_formats
            .iter()
            .any(|f| f.depth == display.root_depth && f.bits_per_pixel == 32);
        if !ok {
            return Err(DesktopError::Capture(format!(
                "X11 screen {idx} root depth {} has no 32bpp format",
                display.root_depth
            )));
        }
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
            recycled: None,
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

    /// Return an encoded frame's owned storage for reuse on the next
    /// capture. A scaled frame has a different size and is ignored.
    pub(crate) fn recycle(&mut self, data: Bytes) {
        let expected = usize::from(self.width) * usize::from(self.height) * 4;
        if self.shm.is_some() && data.len() == expected {
            self.recycled = Some(data);
        }
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
                        data: copy_capture(&mut self.recycled, &shm.map[..shm.len]),
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
        let expected = usize::from(self.width) * usize::from(self.height) * 4;
        if reply.data.len() != expected {
            return Err(DesktopError::Capture(format!(
                "X11 GetImage returned {} bytes for {}x{} (expected {expected})",
                reply.data.len(),
                self.width,
                self.height
            )));
        }
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

/// The X11 segment is overwritten by the next request, so capture must
/// copy into owned storage. Reuse only uniquely owned, correctly sized data;
/// a retained snapshot must remain immutable.
fn copy_capture(recycled: &mut Option<Bytes>, source: &[u8]) -> Bytes {
    if let Some(bytes) = recycled.take()
        && let Ok(mut buffer) = bytes.try_into_mut()
        && buffer.len() == source.len()
    {
        buffer.copy_from_slice(source);
        return buffer.freeze();
    }
    Bytes::copy_from_slice(source)
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
    use super::*;

    #[test]
    fn capture_storage_reuses_unique_buffers_and_preserves_retained_snapshots() {
        let first = Bytes::from(vec![1u8; 1024]);
        let pointer = first.as_ptr();
        let mut recycled = Some(first);
        let next = copy_capture(&mut recycled, &[2u8; 1024]);
        assert_eq!(next.as_ptr(), pointer, "unique buffer must be reused");
        assert!(recycled.is_none());
        assert_eq!(next.as_ref(), &[2u8; 1024]);

        let retained = next.clone();
        recycled = Some(next);
        let changed = copy_capture(&mut recycled, &[3u8; 1024]);
        assert_ne!(changed.as_ptr(), retained.as_ptr());
        assert_eq!(retained.as_ref(), &[2u8; 1024]);
        assert_eq!(changed.as_ref(), &[3u8; 1024]);

        recycled = Some(changed);
        assert_eq!(copy_capture(&mut recycled, &[4u8; 16]).as_ref(), &[4u8; 16]);
    }

    /// Explicit native fixture: never silently succeeds without capture.
    #[test]
    #[ignore = "requires a dedicated Xvfb server; repaints its root"]
    fn capture_roundtrip() {
        let display = std::env::var_os("DISPLAY").expect("dedicated X11 server required");
        let local = display.to_string_lossy().starts_with(':');
        let mut cap = X11Capturer::new(0).expect("native X11 capture must open");
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
        use x11rb::protocol::xproto::{CreateGCAux, Rectangle};
        let gc = cap.conn.generate_id().unwrap();
        cap.conn
            .create_gc(gc, cap.root, &CreateGCAux::new().foreground(0x12_34_56))
            .unwrap()
            .check()
            .unwrap();
        cap.conn
            .poly_fill_rectangle(
                cap.root,
                gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                }],
            )
            .unwrap()
            .check()
            .unwrap();
        assert!(cap.changed(), "root repaint produced no damage");
        let frame = cap.capture().unwrap();
        let pixel = 50 * frame.stride as usize + 50 * 4;
        assert_eq!(
            &frame.data[pixel..pixel + 3],
            &[0x56, 0x34, 0x12],
            "captured pixel did not match the native paint"
        );
        cap.conn.free_gc(gc).unwrap().check().unwrap();
    }
}
