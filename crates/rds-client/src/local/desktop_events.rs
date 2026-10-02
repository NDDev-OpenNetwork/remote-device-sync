//! Same-UID event routes owned by one desktop, never by a peer connection.
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use rds_core::{
    DesktopEvent,
    local::{DesktopDown, ErrorCode, SessionId},
};
use rds_net::write_frame;
use tokio::{
    io::AsyncReadExt,
    net::UnixStream,
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const EVENT_CAPACITY: usize = 128;

struct Pending {
    events: mpsc::Receiver<DesktopEvent>,
    ready: oneshot::Sender<()>,
    stop: CancellationToken,
}

#[derive(Default)]
pub(super) struct Registry(Mutex<BTreeMap<SessionId, Option<Pending>>>);

impl Registry {
    pub(super) fn register(self: &Arc<Self>) -> Result<Registration, ErrorCode> {
        let mut routes = self.0.lock().map_err(|_| ErrorCode::Internal)?;
        if routes.len() >= super::MAX_STREAMS {
            return Err(ErrorCode::Capacity);
        }
        // Never replace an existing route, even on a random-id collision.
        let route = (0..8)
            .map(|_| SessionId(rand::random()))
            .find(|id| !routes.contains_key(id))
            .ok_or(ErrorCode::Capacity)?;
        let (sender, events) = mpsc::channel(EVENT_CAPACITY);
        let (ready, attached) = oneshot::channel();
        let stop = CancellationToken::new();
        routes.insert(
            route,
            Some(Pending {
                events,
                ready,
                stop: stop.clone(),
            }),
        );
        Ok(Registration {
            route,
            sender,
            attached,
            stop,
            registry: Arc::downgrade(self),
        })
    }

    pub(super) fn take(&self, route: SessionId) -> Result<Subscription, ErrorCode> {
        let pending = self
            .0
            .lock()
            .map_err(|_| ErrorCode::Internal)?
            .get_mut(&route)
            .and_then(Option::take)
            .ok_or(ErrorCode::NotFound)?;
        Ok(Subscription {
            events: pending.events,
            ready: Some(pending.ready),
            stop: pending.stop,
        })
    }
}

pub(super) struct Registration {
    pub(super) route: SessionId,
    pub(super) sender: mpsc::Sender<DesktopEvent>,
    attached: oneshot::Receiver<()>,
    pub(super) stop: CancellationToken,
    registry: Weak<Registry>,
}

impl Registration {
    pub(super) async fn attached(&mut self) -> io::Result<()> {
        tokio::time::timeout(ATTACH_TIMEOUT, &mut self.attached)
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "desktop event attachment timed out",
                )
            })?
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "desktop event attachment ended")
            })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(registry) = self.registry.upgrade() {
            registry
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.route);
        }
    }
}

pub(super) struct Subscription {
    events: mpsc::Receiver<DesktopEvent>,
    ready: Option<oneshot::Sender<()>>,
    stop: CancellationToken,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// EOF or unexpected caller bytes terminate this desktop, never its shared peer.
/// The ready fence is after the successful subscription reply write.
pub(super) async fn serve(stream: &mut UnixStream, subscription: Subscription) -> io::Result<()> {
    let (reader, writer) = stream.split();
    serve_io(reader, writer, subscription).await
}

async fn serve_io<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    mut subscription: Subscription,
) -> io::Result<()> {
    if let Some(ready) = subscription.ready.take() {
        ready
            .send(())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "desktop owner ended"))?;
    }
    let mut probe = [0];
    let events = async {
        while let Some(event) = subscription.events.recv().await {
            let started = std::time::Instant::now();
            let result = tokio::time::timeout(
                WRITE_TIMEOUT,
                write_frame(&mut writer, &DesktopDown::Event(event)),
            )
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "desktop event write timed out",
                ))
            });
            if let Err(error) = result {
                tracing::warn!(error_kind = ?error.kind(), elapsed_ms = started.elapsed().as_millis() as u64,
                    "desktop event write failed");
                return Err(error);
            }
        }
        Ok(())
    };
    tokio::select! {
        biased;
        _ = subscription.stop.cancelled() => Ok(()),
        bytes = reader.read(&mut probe) => match bytes? {
            0 => Ok(()),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected desktop event input")),
        },
        result = events => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_bounded_single_claim_and_owner_scoped() {
        let registry = Arc::new(Registry::default());
        let mut owners = Vec::new();
        for _ in 0..super::super::MAX_STREAMS {
            owners.push(registry.register().unwrap());
        }
        assert!(matches!(registry.register(), Err(ErrorCode::Capacity)));
        let route = owners[0].route;
        let subscription = registry.take(route).unwrap();
        assert!(matches!(registry.take(route), Err(ErrorCode::NotFound)));
        assert!(
            matches!(registry.register(), Err(ErrorCode::Capacity)),
            "claimed routes must remain reserved until their owner ends"
        );
        let stop = owners[0].stop.clone();
        drop(owners.remove(0));
        assert!(stop.is_cancelled());
        drop(subscription);
        let remaining = owners[0].route;
        assert!(
            !owners[0].stop.is_cancelled(),
            "another desktop was canceled"
        );
        drop(owners);
        assert!(matches!(registry.take(remaining), Err(ErrorCode::NotFound)));
        assert!(registry.0.lock().unwrap().is_empty());
        assert_eq!(Arc::strong_count(&registry), 1);
    }

    #[test]
    fn abandoned_subscription_cancels_its_owner_before_ready() {
        let registry = Arc::new(Registry::default());
        let mut owner = registry.register().unwrap();
        let subscription = registry.take(owner.route).unwrap();
        assert!(matches!(
            owner.attached.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        drop(subscription);
        assert!(owner.stop.is_cancelled());
        assert!(matches!(
            owner.attached.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn subscription_rejects_extra_input_and_releases_its_route() {
        use tokio::io::AsyncWriteExt;
        let registry = Arc::new(Registry::default());
        let mut owner = registry.register().unwrap();
        let subscription = registry.take(owner.route).unwrap();
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let task = tokio::spawn(async move { serve(&mut server, subscription).await });
        owner.attached().await.unwrap();
        client.write_all(&[1]).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(owner.stop.is_cancelled());
    }
    #[tokio::test]
    async fn blocked_event_writer_has_a_deadline_and_cancels_only_its_owner() {
        let registry = Arc::new(Registry::default());
        let mut owner = registry.register().unwrap();
        let other = registry.register().unwrap();
        let subscription = registry.take(owner.route).unwrap();
        let (server, _unread_client) = tokio::io::duplex(1);
        let (read, write) = tokio::io::split(server);
        let task = tokio::spawn(async move { serve_io(read, write, subscription).await });
        owner.attached().await.unwrap();
        owner
            .sender
            .try_send(DesktopEvent::Heartbeat { seq: 1, ts_ms: 9 })
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(owner.stop.is_cancelled());
        assert!(!other.stop.is_cancelled());
    }
}
