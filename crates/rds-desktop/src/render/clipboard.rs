//! Clipboard state belongs to one focused native window. Network workers only
//! stage offers/chunks; AppKit reads and writes remain on the event-loop thread.
use crate::{
    DesktopError,
    clipboard::{Assembly, MAX_TEXT_BYTES},
};
use rds_core::{ClipboardFormat, DesktopControl};
use std::time::{Duration, Instant};

const COPY_HANDOFF: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Clipboard {
    focused: bool,
    offered: Option<(u64, u32)>,
    requested: Option<(u64, u32, Instant)>,
    assembly: Assembly,
    complete: Option<String>,
    generation: Option<i64>,
    authorized: bool,
    copy_intent: Option<(i64, Instant)>,
}
impl Clipboard {
    pub(super) fn needs_work(&self) -> bool {
        self.complete.is_some()
            || ((self.focused || self.copy_pending())
                && self.offered.is_some()
                && self
                    .requested
                    .as_ref()
                    .is_none_or(|(_, _, at)| at.elapsed() > Duration::from_secs(30)))
    }
    /// Only already authorized work or one explicit Copy can outlive selection.
    /// Native ownership changes supersede both, without reading clipboard text.
    pub(super) fn handoff_pending(&mut self, generation: i64) -> bool {
        if let Some((before, at)) = self.copy_intent {
            if before == generation && at.elapsed() < COPY_HANDOFF {
                return true;
            }
            self.reset();
            return false;
        }
        if !self.authorized {
            return false;
        }
        let pending = self.generation == Some(generation)
            && (self.complete.is_some()
                || self
                    .requested
                    .as_ref()
                    .is_some_and(|(_, _, at)| at.elapsed() < Duration::from_secs(30)));
        if !pending {
            self.reset();
        }
        pending
    }
    pub(super) fn focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused && !self.copy_pending() {
            self.offered = None;
        }
    }
    fn copy_pending(&self) -> bool {
        self.copy_intent
            .is_some_and(|(_, at)| at.elapsed() < COPY_HANDOFF)
    }
    pub(super) fn copying(&mut self, generation: i64) {
        if self.focused {
            self.offered = None;
            self.copy_intent = Some((generation, Instant::now()));
        }
    }
    pub(super) fn reset(&mut self) {
        let focused = self.focused;
        *self = Self::default();
        self.focused = focused;
    }
    pub(super) fn offer(&mut self, id: u64, format: ClipboardFormat, bytes: u32) {
        if (self.focused || self.copy_pending())
            && format == ClipboardFormat::TextUtf8
            && bytes as usize <= MAX_TEXT_BYTES
        {
            self.offered = Some((id, bytes));
        }
    }
    pub(super) fn request(&mut self, generation: i64) -> Option<DesktopControl> {
        if self
            .copy_intent
            .is_some_and(|(before, at)| before != generation || at.elapsed() >= COPY_HANDOFF)
        {
            self.copy_intent = None;
            self.offered = None;
            return None;
        }
        if self
            .requested
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(30))
        {
            self.requested = None;
            self.assembly = Assembly::default();
        }
        if (!self.focused && !self.copy_pending())
            || self.requested.is_some()
            || self.complete.is_some()
        {
            return None;
        }
        let (id, bytes) = self.offered.take()?;
        self.copy_intent = None;
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
        // A newer explicit Copy/Cut supersedes this in-flight snapshot. Drain
        // it without changing the pasteboard, then request the latest offer.
        let permitted =
            self.authorized && self.generation == Some(generation) && self.copy_intent.is_none();
        self.authorized = false;
        self.generation = None;
        permitted.then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_tab_handoff_finishes_without_reselecting_and_never_rearms_itself() {
        let mut old = Clipboard::default();
        old.focus(true);
        old.copying(17);
        old.focus(false);
        assert!(old.handoff_pending(17));
        old.offer(1, ClipboardFormat::TextUtf8, 3);
        assert!(old.request(17).is_some());
        assert!(old.handoff_pending(17));
        old.chunk(1, 0, 3, b"abc".to_vec()).unwrap();
        assert!(old.handoff_pending(17));
        assert_eq!(old.publish(17).as_deref(), Some("abc"));
        assert!(!old.handoff_pending(18));
        old.offer(2, ClipboardFormat::TextUtf8, 3);
        assert!(!old.needs_work());
    }
    #[test]
    fn superseded_native_or_remote_copy_cannot_publish_a_hidden_reply() {
        for newer_local in [false, true] {
            let mut old = Clipboard::default();
            old.focus(true);
            old.copying(17);
            old.offer(1, ClipboardFormat::TextUtf8, 3);
            assert!(old.request(17).is_some());
            old.focus(false);
            if newer_local {
                assert!(!old.handoff_pending(18));
            } else {
                old.reset(); // New explicit Copy in another tab revokes the old lease.
            }
            old.chunk(1, 0, 3, b"old".to_vec()).unwrap();
            assert!(old.publish(18).is_none());
            assert!(!old.handoff_pending(18));
        }
    }
    #[test]
    fn handoff_expiry_releases_ownership_but_does_not_discard_a_fresh_focused_offer() {
        let mut c = Clipboard::default();
        c.focus(true);
        c.offer(1, ClipboardFormat::TextUtf8, 3);
        assert!(!c.handoff_pending(17));
        assert!(c.request(17).is_some());
        c.focus(false);
        c.requested.as_mut().unwrap().2 = Instant::now() - Duration::from_secs(30);
        assert!(!c.handoff_pending(17));
        c.chunk(1, 0, 3, b"old".to_vec()).unwrap();
        assert!(c.publish(17).is_none());
        c.focus(true);
        c.copying(17);
        c.copy_intent.as_mut().unwrap().1 = Instant::now() - COPY_HANDOFF;
        assert!(!c.handoff_pending(17));
    }
    #[test]
    fn explicit_copy_accepts_one_delayed_offer_after_switching_apps() {
        let mut c = Clipboard::default();
        c.focus(true);
        c.copying(17);
        c.focus(false);
        c.offer(1, ClipboardFormat::TextUtf8, 3);
        assert!(c.needs_work());
        assert!(c.request(17).is_some());
        c.chunk(1, 0, 3, b"abc".to_vec()).unwrap();
        assert_eq!(c.publish(17), Some("abc".into()));
        c.offer(2, ClipboardFormat::TextUtf8, 0);
        assert!(c.request(17).is_none());
        assert!(!c.needs_work());
    }
    #[test]
    fn copy_handoff_expires_and_cannot_replace_a_new_local_copy() {
        for expired in [false, true] {
            let mut c = Clipboard::default();
            c.focus(true);
            c.copying(17);
            c.focus(false);
            c.offer(1, ClipboardFormat::TextUtf8, 0);
            if expired {
                c.copy_intent = Some((17, Instant::now() - COPY_HANDOFF));
            }
            assert!(c.request(if expired { 17 } else { 18 }).is_none());
            assert!(c.publish(17).is_none());
            c.copying(18); // Background callers cannot arm another handoff.
            c.offer(2, ClipboardFormat::TextUtf8, 0);
            assert!(c.request(18).is_none());
        }
    }
    #[test]
    fn newer_copy_drains_the_old_transfer_without_publishing_it() {
        let mut c = Clipboard::default();
        c.focus(true);
        c.offer(1, ClipboardFormat::TextUtf8, 3);
        assert!(c.request(9).is_some());
        c.copying(9);
        c.focus(false);
        c.offer(2, ClipboardFormat::TextUtf8, 3);
        c.chunk(1, 0, 3, b"old".to_vec()).unwrap();
        assert!(c.publish(9).is_none());
        assert!(matches!(
            c.request(9),
            Some(DesktopControl::ClipboardRequest { id: 2, .. })
        ));
        c.chunk(2, 0, 3, b"new".to_vec()).unwrap();
        assert_eq!(c.publish(9), Some("new".into()));
    }
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
