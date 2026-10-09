//! Clipboard state belongs to one focused native window. Network workers only
//! stage offers/chunks; AppKit reads and writes remain on the event-loop thread.
use crate::{
    DesktopError,
    clipboard::{Assembly, MAX_TEXT_BYTES},
};
use rds_core::{ClipboardFormat, DesktopControl};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Clipboard {
    focused: bool,
    offered: Option<(u64, u32)>,
    requested: Option<(u64, u32, Instant)>,
    assembly: Assembly,
    complete: Option<String>,
    generation: Option<i64>,
    authorized: bool,
}
impl Clipboard {
    pub(super) fn needs_work(&self) -> bool {
        self.complete.is_some()
            || (self.focused
                && self.offered.is_some()
                && self
                    .requested
                    .as_ref()
                    .is_none_or(|(_, _, at)| at.elapsed() > Duration::from_secs(30)))
    }
    pub(super) fn focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.offered = None;
        }
    }
    pub(super) fn reset(&mut self) {
        let focused = self.focused;
        *self = Self::default();
        self.focused = focused;
    }
    pub(super) fn offer(&mut self, id: u64, format: ClipboardFormat, bytes: u32) {
        if self.focused && format == ClipboardFormat::TextUtf8 && bytes as usize <= MAX_TEXT_BYTES {
            self.offered = Some((id, bytes));
        }
    }
    pub(super) fn request(&mut self, generation: i64) -> Option<DesktopControl> {
        if self
            .requested
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(30))
        {
            self.requested = None;
            self.assembly = Assembly::default();
        }
        if !self.focused || self.requested.is_some() || self.complete.is_some() {
            return None;
        }
        let (id, bytes) = self.offered.take()?;
        self.requested = Some((id, bytes, Instant::now()));
        self.generation = Some(generation);
        self.authorized = true;
        Some(DesktopControl::ClipboardRequest {
            id,
            format: ClipboardFormat::TextUtf8,
        })
    }
    pub(super) fn chunk(
        &mut self,
        id: u64,
        offset: u32,
        total: u32,
        data: Vec<u8>,
    ) -> Result<(), DesktopError> {
        if !self.focused && !self.authorized {
            return Ok(());
        }
        if self.requested.as_ref().is_none_or(|(expected, bytes, at)| {
            *expected != id || *bytes != total || at.elapsed() > Duration::from_secs(30)
        }) {
            return Err(DesktopError::Input(
                "unsolicited or expired clipboard transfer".into(),
            ));
        }
        match self.assembly.push(id, offset, total, data) {
            Ok(Some(text)) => {
                self.complete = Some(text);
                self.requested = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.reset();
                return Err(error);
            }
        }
        Ok(())
    }
    pub(super) fn failed(&mut self, id: u64) {
        if self
            .requested
            .as_ref()
            .is_some_and(|(expected, _, _)| *expected == id)
        {
            self.requested = None;
            self.assembly = Assembly::default();
            self.generation = None;
            self.authorized = false;
        }
    }
    pub(super) fn publish(&mut self, generation: i64) -> Option<String> {
        let value = self.complete.take()?;
        let permitted = self.authorized && self.generation == Some(generation);
        self.authorized = false;
        self.generation = None;
        permitted.then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_windows_and_unsolicited_chunks_never_publish() {
        let mut c = Clipboard::default();
        c.offer(7, ClipboardFormat::TextUtf8, 3);
        assert!(c.request(1).is_none());
        c.focus(true);
        assert!(c.chunk(7, 0, 3, b"abc".to_vec()).is_err());
        assert!(c.publish(1).is_none());
        c.offer(7, ClipboardFormat::TextUtf8, 3);
        assert!(c.request(1).is_some());
        c.focus(false);
        c.chunk(7, 0, 3, b"abc".to_vec()).unwrap();
        assert_eq!(c.publish(1), Some("abc".into()));
    }
    #[test]
    fn complete_utf8_requires_exact_offer_and_unchanged_native_generation() {
        let mut c = Clipboard::default();
        c.focus(true);
        for (id, generation, final_generation, result) in [(1, 9, 9, true), (2, 10, 11, false)] {
            c.offer(id, ClipboardFormat::TextUtf8, 4);
            assert!(c.request(generation).is_some());
            assert!(c.chunk(id + 90, 0, 4, b"fake".to_vec()).is_err());
            assert!(c.chunk(id, 0, 9, vec![0; 9]).is_err());
            c.chunk(id, 0, 4, "🖥".as_bytes()[..2].to_vec()).unwrap();
            c.chunk(id, 2, 4, "🖥".as_bytes()[2..].to_vec()).unwrap();
            assert_eq!(c.publish(final_generation).is_some(), result);
            assert!(c.publish(final_generation).is_none());
        }
        c.offer(3, ClipboardFormat::TextUtf8, MAX_TEXT_BYTES as u32 + 1);
        assert!(c.request(1).is_none());
    }
    #[test]
    fn newer_offer_waits_for_requested_transfer_and_reconnect_clears_both() {
        let mut c = Clipboard::default();
        c.focus(true);
        c.offer(1, ClipboardFormat::TextUtf8, 0);
        assert!(c.request(8).is_some());
        c.offer(2, ClipboardFormat::TextUtf8, 0);
        assert!(c.request(8).is_none());
        c.chunk(1, 0, 0, vec![]).unwrap();
        assert_eq!(c.publish(8), Some(String::new()));
        assert!(matches!(
            c.request(9),
            Some(DesktopControl::ClipboardRequest { id: 2, .. })
        ));
        c.reset();
        assert!(c.request(9).is_none());
        assert!(c.publish(9).is_none());
    }
}
