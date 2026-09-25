//! Bounded retries for path allocation, owned by the existing policy task.
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::time::Instant;

const MAX_PENDING: usize = super::policy::MAX_CANDIDATES + 1 + super::MAX_QNT_ADDRESSES as usize;
const WORK_BATCH: usize = 8;
const WAIT_LIMIT: Duration = Duration::from_secs(15);
const FIRST_RETRY: Duration = Duration::from_millis(25);
const MAX_RETRY: Duration = Duration::from_millis(400);

#[derive(Clone, Copy)]
pub(super) enum Origin {
    Ticket,
    Advertisement,
}

struct Entry {
    ticket: bool,
    deadline: Instant,
    next: Instant,
    backoff: Duration,
}

#[derive(Default)]
pub(super) struct Pending {
    entries: BTreeMap<SocketAddr, Entry>,
}

pub(super) struct Opened {
    pub id: noq::PathId,
    pub learned: bool,
}

enum Attempt {
    Opened(noq::PathId),
    Retry,
    Rejected,
}

fn canonical(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V6(v6) if v6.ip().to_ipv4_mapped().is_some() => {
            SocketAddr::new(v6.ip().to_canonical(), v6.port())
        }
        address => address,
    }
}

impl Pending {
    pub fn offer(&mut self, address: SocketAddr, origin: Origin, now: Instant) {
        let address = canonical(address);
        if let Some(entry) = self.entries.get_mut(&address) {
            entry.ticket |= matches!(origin, Origin::Ticket);
            // Repeated advertisements cannot extend the original budget or
            // turn a resource retry into an unbounded busy loop.
            return;
        }
        if self.entries.len() == MAX_PENDING {
            tracing::debug!(%address, "pending path candidate limit reached");
            return;
        }
        self.entries.insert(
            address,
            Entry {
                ticket: matches!(origin, Origin::Ticket),
                deadline: now + WAIT_LIMIT,
                next: now,
                backoff: FIRST_RETRY,
            },
        );
    }

    pub fn withdraw(&mut self, address: SocketAddr) {
        let address = canonical(address);
        if self
            .entries
            .get(&address)
            .is_some_and(|entry| !entry.ticket)
        {
            self.entries.remove(&address);
        }
    }

    /// Reconcile only peer advertisements; a ticket is an independent source.
    pub fn reconcile(&mut self, addresses: impl IntoIterator<Item = SocketAddr>, now: Instant) {
        let current: BTreeSet<_> = addresses
            .into_iter()
            .take(super::MAX_QNT_ADDRESSES as usize)
            .map(canonical)
            .collect();
        self.entries
            .retain(|address, entry| entry.ticket || current.contains(address));
        for address in current {
            self.offer(address, Origin::Advertisement, now);
        }
    }

    pub fn next_wake(&self) -> Option<Instant> {
        self.entries
            .values()
            .map(|entry| entry.next.min(entry.deadline))
            .min()
    }

    pub fn open_due(&mut self, connection: &noq::Connection, now: Instant) -> Vec<Opened> {
        self.poll_due(now, |address| attempt(connection, address))
    }

    fn poll_due(
        &mut self,
        now: Instant,
        mut attempt: impl FnMut(SocketAddr) -> Attempt,
    ) -> Vec<Opened> {
        self.entries.retain(|_, entry| now < entry.deadline);
        let mut remove = Vec::with_capacity(WORK_BATCH);
        let mut opened = Vec::with_capacity(WORK_BATCH);
        for (&address, entry) in self
            .entries
            .iter_mut()
            .filter(|(_, entry)| entry.next <= now)
            .take(WORK_BATCH)
        {
            match attempt(address) {
                Attempt::Opened(id) => {
                    opened.push(Opened {
                        id,
                        learned: !entry.ticket,
                    });
                    remove.push(address);
                }
                Attempt::Rejected => remove.push(address),
                Attempt::Retry => {
                    entry.next = now + entry.backoff;
                    entry.backoff = (entry.backoff * 2).min(MAX_RETRY);
                }
            }
        }
        for address in remove {
            self.entries.remove(&address);
        }
        opened
    }
}

fn attempt(connection: &noq::Connection, address: SocketAddr) -> Attempt {
    let mut open = connection.open_path_ensure(address, noq::PathStatus::Backup);
    if let Some(id) = open.path_id() {
        // Allocation is not validation. Drop OpenPath promptly; Established
        // is still the only admission signal for application path selection.
        return Attempt::Opened(id);
    }
    // With no allocated ID, noq documents an immediately-ready rejection.
    // Poll synchronously to classify it without retaining a connection across
    // an await. A future API change to Pending is still deadline-bounded.
    match Pin::new(&mut open).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Err(
            noq::PathError::RemoteCidsExhausted | noq::PathError::MaxPathIdReached,
        )) => Attempt::Retry,
        Poll::Ready(Err(error)) => {
            tracing::debug!(%address, %error, "path candidate permanently rejected");
            Attempt::Rejected
        }
        Poll::Ready(Ok(path)) => Attempt::Opened(path.id()),
        Poll::Pending => Attempt::Retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn addr(port: u16) -> SocketAddr {
        ([192, 0, 2, 1], port).into()
    }

    #[test]
    fn candidate_capacity_and_duplicate_lifetime_are_bounded() {
        let now = Instant::now();
        let mut pending = Pending::default();
        for index in 0..MAX_PENDING + 5 {
            pending.offer(addr(10000 + index as u16), Origin::Advertisement, now);
        }
        assert_eq!(pending.entries.len(), MAX_PENDING);
        pending.offer(
            addr(10000),
            Origin::Advertisement,
            now + Duration::from_secs(14),
        );
        assert!(
            pending
                .poll_due(now + WAIT_LIMIT, |_| panic!("expired candidate attempted"))
                .is_empty()
        );
        assert!(pending.next_wake().is_none());
    }

    #[test]
    fn retries_back_off_without_duplicate_acceleration() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.offer(addr(1), Origin::Ticket, now);
        let mut calls = 0;
        pending.poll_due(now, |_| {
            calls += 1;
            Attempt::Retry
        });
        assert_eq!(calls, 1);
        assert_eq!(pending.next_wake(), Some(now + FIRST_RETRY));
        pending.offer(
            addr(1),
            Origin::Advertisement,
            now + Duration::from_millis(1),
        );
        pending.poll_due(now + Duration::from_millis(24), |_| {
            panic!("retry accelerated")
        });
        pending.poll_due(now + FIRST_RETRY, |_| Attempt::Retry);
        assert_eq!(pending.next_wake(), Some(now + Duration::from_millis(75)));
        let mut cursor = now + Duration::from_millis(75);
        for _ in 0..10 {
            pending.poll_due(cursor, |_| Attempt::Retry);
            let next = pending.next_wake().unwrap();
            assert!(next - cursor <= MAX_RETRY);
            cursor = next;
        }
        pending.poll_due(now + WAIT_LIMIT, |_| panic!("retry outlived deadline"));
        assert!(pending.next_wake().is_none());
    }

    #[test]
    fn reconciliation_withdraws_advertisements_and_preserves_tickets() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.offer(addr(1), Origin::Ticket, now);
        pending.offer(addr(2), Origin::Advertisement, now);
        pending.reconcile([addr(3)], now);
        assert!(pending.entries.contains_key(&addr(1)));
        assert!(!pending.entries.contains_key(&addr(2)));
        assert!(pending.entries.contains_key(&addr(3)));
        pending.withdraw(addr(1));
        pending.withdraw(addr(3));
        let opened = pending.poll_due(now, |_| Attempt::Opened(noq::PathId::from(1u32)));
        assert_eq!(opened.len(), 1);
        assert!(!opened[0].learned);
        assert!(pending.next_wake().is_none());
    }

    #[test]
    fn due_work_is_batched_and_permanent_errors_stop_retrying() {
        let now = Instant::now();
        let mut pending = Pending::default();
        for index in 0..MAX_PENDING {
            pending.offer(addr(10000 + index as u16), Origin::Ticket, now);
        }
        let mut calls = 0;
        while pending.next_wake().is_some() {
            let before = calls;
            pending.poll_due(now, |_| {
                calls += 1;
                Attempt::Rejected
            });
            assert!(calls - before <= WORK_BATCH);
        }
        assert_eq!(calls, MAX_PENDING);
    }

    #[test]
    fn mapped_ipv4_has_one_pending_entry_and_one_open() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.offer(addr(1), Origin::Advertisement, now);
        pending.offer(
            "[::ffff:192.0.2.1]:1".parse().unwrap(),
            Origin::Advertisement,
            now,
        );
        assert_eq!(pending.entries.len(), 1);
        let opened = pending.poll_due(now, |_| Attempt::Opened(noq::PathId::from(1u32)));
        assert_eq!(opened.len(), 1);
        assert!(opened[0].learned);
        assert!(pending.next_wake().is_none());
    }
}
