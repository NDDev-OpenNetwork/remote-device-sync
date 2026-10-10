//! One newest CPU image per session, shared with its pending measurement.
use super::{Arc, Instant, RawFrame};

pub(super) struct Pending {
    pub raw: Arc<RawFrame>,
    pub received: Instant,
    pub frame_seq: Option<u64>,
    pub queued_ms: u64,
}

#[derive(Default)]
pub(super) struct FrameMailbox {
    pub pending: Option<Pending>,
    latest: Option<Arc<RawFrame>>,
}

impl FrameMailbox {
    pub fn queue(&mut self, frame: Pending) -> bool {
        self.latest = Some(frame.raw.clone());
        self.pending.replace(frame).is_some()
    }

    /// Restoring an idle tab supplies pixels, never a second timing sample.
    pub fn take(&mut self, has_gpu_picture: bool) -> (Option<Pending>, Option<Arc<RawFrame>>) {
        let pending = self.pending.take();
        let restore = if pending.is_none() && !has_gpu_picture {
            self.latest.clone()
        } else {
            None
        };
        (pending, restore)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(value: u8) -> Pending {
        Pending {
            raw: Arc::new(RawFrame {
                width: 1,
                height: 1,
                stride: 4,
                data: vec![value, value, value, 255].into(),
            }),
            received: Instant::now(),
            frame_seq: Some(u64::from(value)),
            queued_ms: 0,
        }
    }

    #[test]
    fn idle_tab_restores_shared_pixels_without_replaying_frame_timing() {
        let mut frames = FrameMailbox::default();
        let frame = pending(1);
        let identity = Arc::downgrade(&frame.raw);
        assert!(!frames.queue(frame));
        let (first, restore) = frames.take(false);
        assert_eq!(first.as_ref().unwrap().frame_seq, Some(1));
        assert!(restore.is_none());
        drop(first);
        assert!(frames.take(true).1.is_none()); // ordinary UI redraw needs no upload
        for _ in 0..8 {
            let (measurement, restore) = frames.take(false); // return from another tab
            assert!(measurement.is_none());
            assert!(Arc::ptr_eq(&restore.unwrap(), &identity.upgrade().unwrap()));
        }
        assert!(!frames.queue(pending(2)));
        assert!(identity.upgrade().is_none()); // no history or pixel-buffer copy
        assert!(frames.queue(pending(3))); // hidden newest-frame replacement
        let (next, restore) = frames.take(false);
        assert_eq!(next.unwrap().frame_seq, Some(3));
        assert!(restore.is_none());
        assert_eq!(frames.take(false).1.unwrap().data[0], 3);
        frames = FrameMailbox::default(); // a new connection cannot restore old pixels
        assert!(frames.take(false).1.is_none());
    }
}
