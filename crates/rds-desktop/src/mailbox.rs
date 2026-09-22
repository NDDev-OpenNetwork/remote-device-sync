//! Bounded newest-wins queue.
//!
//! `mpsc` drops the *newest* item when full (`try_send` fails), which is
//! the wrong direction for a latency-sensitive pipeline: a queued stale
//! frame is pure latency, the fresh one is the information. [`Mailbox`]
//! evicts the oldest queued item instead, so depth stays bounded and the
//! consumer always sees the freshest available item.

use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::Notify;

/// Shared state for one queue; both ends clone the `Arc`.
struct State<T> {
    queue: Mutex<VecDeque<T>>,
    notify: Notify,
    capacity: usize,
}

/// Producer half: `send` never blocks and never fails while a receiver
/// half exists; on a full queue the oldest item is evicted.
pub struct Sender<T>(std::sync::Arc<State<T>>);

/// Receiver half: `recv` awaits items; `try_recv` is non-blocking.
pub struct Receiver<T>(std::sync::Arc<State<T>>);

/// Create a queue pair that holds at most `capacity` items.
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let state = std::sync::Arc::new(State {
        queue: Mutex::new(VecDeque::with_capacity(capacity)),
        notify: Notify::new(),
        capacity: capacity.max(1),
    });
    (Sender(state.clone()), Receiver(state))
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Sender<T> {
    /// Enqueue `item`; evicts the oldest queued item when full.
    /// Returns the evicted item, if any.
    pub fn send(&self, item: T) -> Option<T> {
        let mut evicted = None;
        {
            let mut q = self.0.queue.lock().unwrap();
            if q.len() >= self.0.capacity {
                evicted = q.pop_front();
            }
            q.push_back(item);
        }
        self.0.notify.notify_one();
        evicted
    }

    /// Queued depth right now (test/diagnostic visibility).
    pub fn len(&self) -> usize {
        self.0.queue.lock().unwrap().len()
    }

    /// True when nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.0.queue.lock().unwrap().is_empty()
    }
}

impl<T> Receiver<T> {
    /// Next item, awaiting when empty. Returns `None` when every sender
    /// is gone — a dropped queue reads as closed.
    pub async fn recv(&mut self) -> Option<T> {
        loop {
            {
                let mut q = self.0.queue.lock().unwrap();
                if let Some(item) = q.pop_front() {
                    return Some(item);
                }
            }
            // All senders dropped → only our own Arc remains.
            if std::sync::Arc::strong_count(&self.0) == 1 {
                return None;
            }
            self.0.notify.notified().await;
        }
    }

    /// Non-blocking pop; `None` when empty.
    pub fn try_recv(&mut self) -> Option<T> {
        self.0.queue.lock().unwrap().pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn full_queue_evicts_oldest() {
        let (tx, mut rx) = channel(2);
        tx.send(1);
        tx.send(2);
        tx.send(3); // evicts 1
        assert_eq!(rx.recv().await, Some(2));
        assert_eq!(rx.recv().await, Some(3));
    }

    #[tokio::test]
    async fn closed_when_senders_gone() {
        let (tx, mut rx) = channel::<u8>(2);
        tx.send(1);
        drop(tx);
        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn recv_awaits_produce() {
        let (tx, mut rx) = channel(4);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            tx.send(7);
        });
        assert_eq!(rx.recv().await, Some(7));
    }
}
