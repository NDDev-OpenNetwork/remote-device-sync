//! Bounded reference ordering. A delta completing first cannot invalidate a
//! keyframe still in flight. Only independently decodable frames can skip a gap.
use rds_core::FrameHeader;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(crate) struct Ordered<T> {
    next: Option<u64>,
    frames: BTreeMap<u64, (FrameHeader, T)>,
    blocked: Option<Instant>,
}
impl<T> Default for Ordered<T> {
    fn default() -> Self {
        Self {
            next: None,
            frames: BTreeMap::new(),
            blocked: None,
        }
    }
}
impl<T> Ordered<T> {
    pub fn push(&mut self, header: FrameHeader, payload: T) {
        if header.seq == u64::MAX || self.next.is_some_and(|next| header.seq < next) {
            return;
        }
        if header.keyframe && self.next.is_none_or(|next| header.seq > next) {
            self.frames.retain(|seq, _| *seq >= header.seq);
            self.next = Some(header.seq);
            self.blocked = None;
        }
        // Retain the nearest three successors, leaving global reader permits
        // available for the missing reference or a replacement keyframe.
        self.frames.insert(header.seq, (header, payload));
        // Three waiting successors plus their now-complete reference fit
        // the four reader permits. Do not evict a valid successor exactly
        // when its missing reference arrives; pop will remove it next.
        let limit = 3 + usize::from(
            self.next
                .is_some_and(|next| self.frames.contains_key(&next)),
        );
        while self.frames.len() > limit {
            self.frames.pop_last();
        }
        if self.blocked.is_none() {
            self.blocked = Some(Instant::now());
        }
    }
    pub fn pop(&mut self) -> Option<(FrameHeader, T)> {
        let next = self.next?;
        let frame = self.frames.remove(&next)?;
        self.next = next.checked_add(1);
        self.blocked = if self.frames.is_empty() {
            None
        } else {
            Some(Instant::now())
        };
        Some(frame)
    }
    pub fn expire(&mut self, timeout: Duration) -> bool {
        if self.blocked.is_some_and(|at| at.elapsed() >= timeout) {
            self.frames.clear();
            self.next = None;
            self.blocked = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header(seq: u64, keyframe: bool) -> FrameHeader {
        FrameHeader {
            seq,
            keyframe,
            capture_ts_ms: 0,
            encode_done_ts_ms: 0,
            send_ts_ms: 0,
            codec: rds_core::Codec::H264,
            width: 32,
            height: 32,
        }
    }
    #[test]
    fn completing_the_missing_reference_retains_all_three_admitted_successors() {
        let mut order = Ordered::default();
        order.push(header(0, true), 0);
        order.pop();
        for seq in [2, 3, 4] {
            order.push(header(seq, false), seq);
        }
        order.push(header(1, false), 1);
        for expected in 1..=4 {
            assert_eq!(order.pop().unwrap().0.seq, expected);
        }
        assert!(order.pop().is_none());
    }

    #[test]
    fn a_later_delta_cannot_discard_the_initial_keyframe() {
        let mut order = Ordered::default();
        order.push(header(1, false), "delta");
        assert!(order.pop().is_none());
        order.push(header(0, true), "keyframe");
        assert_eq!(order.pop().unwrap().1, "keyframe");
        assert_eq!(order.pop().unwrap().1, "delta");
    }
    #[test]
    fn reordering_waits_for_references_but_an_independent_frame_recovers() {
        let mut order = Ordered::default();
        order.push(header(0, true), 0);
        order.pop();
        order.push(header(2, false), 2);
        assert!(order.pop().is_none());
        order.push(header(1, false), 1);
        assert_eq!(order.pop().unwrap().1, 1);
        assert_eq!(order.pop().unwrap().1, 2);
        order.push(header(4, false), 4);
        assert!(order.pop().is_none());
        order.push(header(5, true), 5);
        assert_eq!(order.pop().unwrap().1, 5);
        order.push(header(3, false), 3);
        assert!(order.pop().is_none());
        assert!(order.frames.is_empty());
    }
}
