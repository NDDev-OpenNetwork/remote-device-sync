//! One X11 inventory and selection boundary shared by capture and input.
//! Legacy IDs select X roots; monitor IDs are stable hashes of root number and
//! RandR monitor name, with collisions refused rather than silently aliased.
use crate::DesktopError;
use rds_core::DisplayInfo;
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        randr::{self, ConnectionExt as _},
        xproto::{ConnectionExt as _, Window},
    },
    rust_connection::RustConnection,
};

pub(crate) const MONITOR_BIT: u32 = 1 << 31;
const MAX_DISPLAYS: usize = 64;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Display {
    pub(crate) id: u32,
    pub(crate) root: Window,
    pub(crate) x: i16,
    pub(crate) y: i16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) primary: bool,
    pub(crate) root_depth: u8,
    pub(crate) monitor: Option<u32>,
}
impl Display {
    pub(crate) fn info(&self) -> DisplayInfo {
        DisplayInfo {
            index: self.id,
            width: self.width.into(),
            height: self.height.into(),
            primary: self.primary,
        }
    }
}
fn error(reason: impl std::fmt::Display) -> DesktopError {
    DesktopError::Capture(format!("X11 display inventory: {reason}"))
}
fn monitor_id(screen: u32, name: &[u8]) -> u32 {
    let mut key = b"rds-x11-monitor/v1\0".to_vec();
    key.extend_from_slice(&screen.to_le_bytes());
    key.extend_from_slice(name);
    let hash = blake3::hash(&key);
    MONITOR_BIT
        | (u32::from_le_bytes(hash.as_bytes()[..4].try_into().expect("four hash bytes"))
            & !MONITOR_BIT)
}
pub(crate) fn catalog(conn: &RustConnection) -> Result<Vec<Display>, DesktopError> {
    let roots = &conn.setup().roots;
    if roots.is_empty() || roots.len() > MAX_DISPLAYS {
        return Err(error("root count outside bound"));
    }
    let randr = if conn
        .extension_information(randr::X11_EXTENSION_NAME)
        .map_err(error)?
        .is_some()
    {
        let v = conn
            .randr_query_version(1, 5)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        (v.major_version, v.minor_version) >= (1, 5)
    } else {
        false
    };
    let mut result = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        let geometry = conn
            .get_geometry(root.root)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        result.push(Display {
            id: index as u32,
            root: root.root,
            x: 0,
            y: 0,
            width: geometry.width,
            height: geometry.height,
            primary: index == 0,
            root_depth: root.root_depth,
            monitor: None,
        });
        if randr {
            let reply = conn
                .randr_get_monitors(root.root, true)
                .map_err(error)?
                .reply()
                .map_err(error)?;
            for m in reply.monitors {
                if result.len() == MAX_DISPLAYS {
                    return Err(error("monitor count outside bound"));
                }
                if m.x < 0
                    || m.y < 0
                    || m.width == 0
                    || m.height == 0
                    || i32::from(m.x) + i32::from(m.width) > i32::from(geometry.width)
                    || i32::from(m.y) + i32::from(m.height) > i32::from(geometry.height)
                {
                    return Err(error("monitor rectangle outside root"));
                }
                let name = conn
                    .get_atom_name(m.name)
                    .map_err(error)?
                    .reply()
                    .map_err(error)?
                    .name;
                if name.is_empty() || name.len() > 256 {
                    return Err(error("invalid monitor name"));
                }
                let id = monitor_id(index as u32, &name);
                if result.iter().any(|display| display.id == id) {
                    return Err(error("monitor identity collision"));
                }
                result.push(Display {
                    id,
                    root: root.root,
                    x: m.x,
                    y: m.y,
                    width: m.width,
                    height: m.height,
                    primary: m.primary,
                    root_depth: root.root_depth,
                    monitor: Some(m.name),
                });
            }
        }
    }
    result.sort_by_key(|d| d.id);
    Ok(result)
}
pub(crate) fn select(conn: &RustConnection, id: u32) -> Result<Display, DesktopError> {
    catalog(conn)?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| error("requested display unavailable"))
}
pub(crate) fn still_matches(conn: &RustConnection, selected: &Display) -> Result<(), DesktopError> {
    if let Some(name) = selected.monitor {
        let monitors = conn
            .randr_get_monitors(selected.root, true)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        let matches = monitors.monitors.into_iter().any(|m| {
            m.name == name
                && m.x == selected.x
                && m.y == selected.y
                && m.width == selected.width
                && m.height == selected.height
                && m.primary == selected.primary
        });
        return if matches {
            Ok(())
        } else {
            Err(error("selected monitor was removed or reconfigured"))
        };
    }
    if select(conn, selected.id)? != *selected {
        return Err(error("selected display was reconfigured"));
    }
    Ok(())
}
pub(crate) fn subscribe(conn: &RustConnection, root: Window) -> Result<bool, DesktopError> {
    if conn
        .extension_information(randr::X11_EXTENSION_NAME)
        .map_err(error)?
        .is_none()
    {
        return Ok(false);
    }
    conn.randr_query_version(1, 5)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    conn.randr_select_input(
        root,
        randr::NotifyMask::SCREEN_CHANGE
            | randr::NotifyMask::CRTC_CHANGE
            | randr::NotifyMask::OUTPUT_CHANGE
            | randr::NotifyMask::RESOURCE_CHANGE,
    )
    .map_err(error)?
    .check()
    .map_err(error)?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_identity_is_geometry_independent_and_separate_from_legacy_screens() {
        assert!(monitor_id(0, b"DP-1") >= MONITOR_BIT);
        assert_eq!(monitor_id(0, b"DP-1"), monitor_id(0, b"DP-1"));
        assert_ne!(monitor_id(0, b"DP-1"), monitor_id(1, b"DP-1"));
        assert_ne!(monitor_id(0, b"DP-1"), monitor_id(0, b"HDMI-1"));
    }
}
