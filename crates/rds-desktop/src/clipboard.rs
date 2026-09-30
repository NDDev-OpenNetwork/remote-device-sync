//! Explicit text paste, bounded reassembly and session-owned OS clipboard.
use crate::DesktopError;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const CHUNK_BYTES: usize = 32 * 1024;

#[derive(Default)]
pub(crate) struct Assembly {
    partial: Option<(u64, usize, Vec<u8>, std::time::Instant)>,
}
impl Assembly {
    pub(crate) fn push(
        &mut self,
        id: u64,
        offset: u32,
        total: u32,
        data: Vec<u8>,
    ) -> Result<Option<String>, DesktopError> {
        let bad = || DesktopError::Input("invalid or expired clipboard transfer".into());
        let total = total as usize;
        if total > MAX_TEXT_BYTES || data.len() > CHUNK_BYTES {
            self.partial = None;
            return Err(bad());
        }
        if offset == 0 {
            if self.partial.is_some() {
                self.partial = None;
                return Err(bad());
            }
            self.partial = Some((
                id,
                total,
                Vec::with_capacity(total),
                std::time::Instant::now(),
            ));
        }
        let Some((current, length, bytes, started)) = &mut self.partial else {
            return Err(bad());
        };
        if *current != id
            || *length != total
            || bytes.len() != offset as usize
            || bytes.len() + data.len() > total
            || started.elapsed() > std::time::Duration::from_secs(5)
        {
            self.partial = None;
            return Err(bad());
        }
        bytes.extend_from_slice(&data);
        if bytes.len() != total {
            return Ok(None);
        }
        let (_, _, bytes, _) = self.partial.take().expect("complete transfer exists");
        String::from_utf8(bytes).map(Some).map_err(|_| bad())
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
