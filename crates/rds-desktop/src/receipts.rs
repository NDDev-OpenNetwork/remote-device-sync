//! Session-owned, bounded proofs of complete encoded-payload receipt.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    Transport,
    Payload,
    Obsolete,
}

pub(crate) async fn wait(
    ticket: &mut Option<Ticket>,
    transport: impl Future<Output = Result<Evidence, String>>,
) -> Result<Evidence, String> {
    let Some(ticket) = ticket else {
        return transport.await;
    };
    tokio::pin!(transport);
    tokio::select! {
        biased;
        proof = ticket.received() => proof.map_err(|e| e.to_string()),
        result = &mut transport => match result {
            Ok(Evidence::Transport) => ticket.received().await.map_err(|e| e.to_string()),
            other => other,
        }
    }
}

type Pending = BTreeMap<u64, ([u8; 32], Option<oneshot::Sender<Evidence>>)>;

#[derive(Clone)]
pub(crate) struct Receipts(Arc<Mutex<Pending>>);

impl Receipts {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(BTreeMap::new())))
    }
    pub(crate) fn register(&self, seq: u64, digest: [u8; 32]) -> Option<Ticket> {
        let mut pending = self.0.lock().ok()?;
        if pending.len() >= crate::session::MAX_PENDING_FRAME_ACKS || pending.contains_key(&seq) {
            return None;
        }
        let (send, receive) = oneshot::channel();
        pending.insert(seq, (digest, Some(send)));
        Some(Ticket {
            owner: self.clone(),
            seq,
            receive,
        })
    }
    pub(crate) fn confirm(&self, seq: u64, digest: &[u8; 32], obsolete: bool) -> bool {
        let Ok(mut pending) = self.0.lock() else {
            return false;
        };
        let Some((expected, send)) = pending.get_mut(&seq) else {
            return false;
        };
        if expected != digest {
            return false;
        }
        send.take().is_some_and(|send| {
            send.send(if obsolete {
                Evidence::Obsolete
            } else {
                Evidence::Payload
            })
            .is_ok()
        })
    }
}

pub(crate) struct Ticket {
    owner: Receipts,
    seq: u64,
    receive: oneshot::Receiver<Evidence>,
}
impl Ticket {
    pub(crate) async fn received(&mut self) -> Result<Evidence, oneshot::error::RecvError> {
        (&mut self.receive).await
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.owner.0.lock() {
            pending.remove(&self.seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_an_exact_live_payload_proof_completes_once() {
        let receipts = Receipts::new();
        let digest = *blake3::hash(b"controlled payload").as_bytes();
        let mut ticket = receipts.register(7, digest).unwrap();
        assert!(!receipts.confirm(6, &digest, false));
        assert!(!receipts.confirm(7, &[0; 32], false));
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, ticket.received())
                .await
                .is_err()
        );
        assert!(receipts.confirm(7, &digest, false));
        assert!(!receipts.confirm(7, &digest, false));
        ticket.received().await.unwrap();
        drop(ticket);
        assert!(!receipts.confirm(7, &digest, false));
        assert!(receipts.register(7, digest).is_some());
    }
    #[test]
    fn pending_proofs_are_bounded_and_cancellation_frees_the_exact_slot() {
        let receipts = Receipts::new();
        let mut tickets: Vec<_> = (0..crate::session::MAX_PENDING_FRAME_ACKS)
            .map(|seq| receipts.register(seq as u64, [seq as u8; 32]).unwrap())
            .collect();
        assert!(receipts.register(0, [9; 32]).is_none());
        assert!(receipts.register(100, [9; 32]).is_none());
        let retired = tickets.pop().unwrap();
        let seq = retired.seq;
        drop(retired);
        assert!(!receipts.confirm(seq, &[seq as u8; 32], false));
        assert!(receipts.register(100, [9; 32]).is_some());
        drop(tickets);
        assert!(receipts.0.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn a_delayed_transport_fin_cannot_hold_a_validated_payload_ticket() {
        let receipts = Receipts::new();
        let digest = *blake3::hash(b"already received frame").as_bytes();
        let mut ticket = Some(receipts.register(1796, digest).unwrap());
        let transport = std::future::pending::<Result<Evidence, String>>();
        assert!(receipts.confirm(1796, &digest, false));
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                wait(&mut ticket, transport)
            )
            .await
            .unwrap()
            .unwrap(),
            Evidence::Payload
        );
        drop(ticket);
        assert!(receipts.0.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn transport_fin_alone_is_not_a_negotiated_payload_proof() {
        let receipts = Receipts::new();
        let mut ticket = Some(receipts.register(1, [1; 32]).unwrap());
        assert!(
            tokio::time::timeout(
                std::time::Duration::ZERO,
                wait(&mut ticket, async { Ok(Evidence::Transport) })
            )
            .await
            .is_err()
        );
        assert!(receipts.confirm(1, &[1; 32], false));
        assert_eq!(
            wait(&mut ticket, async { Ok(Evidence::Transport) })
                .await
                .unwrap(),
            Evidence::Payload
        );
        let mut legacy = None;
        assert_eq!(
            wait(&mut legacy, async { Ok(Evidence::Transport) })
                .await
                .unwrap(),
            Evidence::Transport
        );
    }
    #[tokio::test]
    async fn obsolete_payload_proofs_and_session_retirement_do_not_confirm_fresh_work() {
        let old = Receipts::new();
        let digest = [9; 32];
        let mut ticket = Some(old.register(0, digest).unwrap());
        assert!(old.confirm(0, &digest, true));
        assert_eq!(
            wait(&mut ticket, std::future::pending()).await.unwrap(),
            Evidence::Obsolete
        );
        drop(ticket);
        let fresh = Receipts::new();
        let mut ticket = fresh.register(0, digest).unwrap();
        assert!(!old.confirm(0, &digest, false));
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, ticket.received())
                .await
                .is_err()
        );
        assert!(fresh.confirm(0, &digest, false));
        assert_eq!(ticket.received().await.unwrap(), Evidence::Payload);
    }
}
