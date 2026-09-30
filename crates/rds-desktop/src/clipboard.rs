//! Explicit text paste, bounded reassembly and session-owned OS clipboard.
use crate::DesktopError;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const SEND_CHUNK_BYTES: usize = 16 * 1024;
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct Partial {
    id: u64,
    total: usize,
    bytes: Vec<u8>,
    started: std::time::Instant,
    progressed: std::time::Instant,
}

#[derive(Default)]
pub(crate) struct Assembly {
    partial: Option<Partial>,
}
impl Assembly {
    pub(crate) fn push(
        &mut self,
        id: u64,
        offset: u32,
        total: u32,
        data: Vec<u8>,
    ) -> Result<Option<String>, DesktopError> {
        let bad =
            |reason: &str| DesktopError::Input(format!("clipboard transfer refused: {reason}"));
        let total = total as usize;
        if total > MAX_TEXT_BYTES || data.len() > CHUNK_BYTES {
            self.partial = None;
            return Err(bad("size bound exceeded"));
        }
        if offset == 0 {
            if self.partial.is_some() {
                self.partial = None;
                return Err(bad("overlapping transfer"));
            }
            let now = std::time::Instant::now();
            self.partial = Some(Partial {
                id,
                total,
                bytes: Vec::with_capacity(total),
                started: now,
                progressed: now,
            });
        }
        let Some(partial) = &mut self.partial else {
            return Err(bad("missing initial chunk"));
        };
        let reason = if partial.id != id || partial.total != total {
            Some("identity or length changed")
        } else if partial.bytes.len() != offset as usize || partial.bytes.len() + data.len() > total
        {
            Some("invalid chunk offset")
        } else if partial.started.elapsed() > TRANSFER_TIMEOUT {
            Some("total deadline exceeded")
        } else if partial.progressed.elapsed() > IDLE_TIMEOUT {
            Some("idle deadline exceeded")
        } else if data.is_empty() && total != 0 {
            Some("empty progress chunk")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.partial = None;
            return Err(bad(reason));
        }
        partial.bytes.extend_from_slice(&data);
        partial.progressed = std::time::Instant::now();
        if partial.bytes.len() != total {
            return Ok(None);
        }
        let partial = self.partial.take().expect("complete transfer exists");
        String::from_utf8(partial.bytes)
            .map(Some)
            .map_err(|_| bad("invalid UTF-8"))
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
mod x11;
#[cfg(all(target_os = "linux", feature = "x11"))]
pub(crate) use x11::Worker;

#[cfg(not(all(target_os = "linux", feature = "x11")))]
pub(crate) struct Worker;
#[cfg(not(all(target_os = "linux", feature = "x11")))]
impl Worker {
    pub(crate) fn new(_: u32) -> Self {
        Self
    }
    pub(crate) async fn publish(&mut self, _: String) -> Result<(), DesktopError> {
        Err(DesktopError::Input("clipboard backend unavailable".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progressing_transfer_survives_original_deadline_but_idle_and_total_are_bounded() {
        let mut assembly = Assembly::default();
        assembly.push(1, 0, 3, vec![b'a']).unwrap();
        assembly.partial.as_mut().unwrap().started -= std::time::Duration::from_secs(6);
        assert!(assembly.push(1, 1, 3, vec![b'b']).unwrap().is_none());
        assert_eq!(
            assembly.push(1, 2, 3, vec![b'c']).unwrap(),
            Some("abc".into())
        );
        assembly.push(2, 0, 2, vec![b'a']).unwrap();
        assembly.partial.as_mut().unwrap().progressed -= std::time::Duration::from_secs(6);
        assert!(
            assembly
                .push(2, 1, 2, vec![b'b'])
                .unwrap_err()
                .to_string()
                .contains("idle deadline")
        );
        assert!(assembly.partial.is_none());
        assembly.push(3, 0, 2, vec![b'a']).unwrap();
        assembly.partial.as_mut().unwrap().started -= std::time::Duration::from_secs(31);
        assert!(
            assembly
                .push(3, 1, 2, vec![b'b'])
                .unwrap_err()
                .to_string()
                .contains("total deadline")
        );
        assert!(assembly.partial.is_none());
        assert!(assembly.push(4, 0, 1, vec![]).is_err());
        assert_eq!(assembly.push(5, 0, 0, vec![]).unwrap(), Some(String::new()));
    }
    #[test]
    fn split_utf8_is_preserved_and_bad_offsets_or_size_fail_closed() {
        let text = "Привет 🖥️\nSecond line".as_bytes();
        let mut a = Assembly::default();
        assert!(
            a.push(1, 0, text.len() as u32, text[..3].to_vec())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            a.push(1, 3, text.len() as u32, text[3..].to_vec()).unwrap(),
            Some(String::from_utf8(text.to_vec()).unwrap())
        );
        assert!(a.push(2, 0, MAX_TEXT_BYTES as u32 + 1, vec![]).is_err());
        assert!(a.push(2, 0, 8, vec![1, 2]).unwrap().is_none());
        assert!(a.push(2, 3, 8, vec![3]).is_err());
        assert!(a.push(3, 0, 1, vec![0xff]).is_err());
    }
}
