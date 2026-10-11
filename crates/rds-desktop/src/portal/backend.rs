//! A prepared consented desktop, owned by the local agent for its lifetime.

use super::{capture, input, producer, session, state};
use crate::{DesktopError, DesktopSource, FrameProducer, InputSink};
use rds_core::{Codec, DesktopCaps, DisplayInfo};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{Semaphore, oneshot, watch};

/// GNOME/KDE portal serving. Opening may present a LOCAL permission dialog.
/// Remote session requests only borrow this prepared backend. Closing/revoking
/// it seals all leases; it never silently falls back to Xwayland or reopens.
pub struct WaylandDesktop {
    slots: Vec<Arc<capture::Slot>>,
    input: Arc<input::Group>,
    producers: Arc<Semaphore>,
    closed: watch::Receiver<bool>,
    close: Mutex<Option<oneshot::Sender<()>>>,
    finished: watch::Receiver<bool>,
}
fn error() -> DesktopError {
    DesktopError::Capture("Wayland desktop is not ready or was revoked".into())
}
impl WaylandDesktop {
    /// Store lives in a private 0700 directory; tokens are never logged.
    pub async fn open(path: &Path) -> Result<Self, DesktopError> {
        let path = path.to_owned();
        static FILE_WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let permit = FILE_WORKERS
            .get_or_init(|| Arc::new(Semaphore::new(4)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| error())?;
        let mut store = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            state::Store::open(&path)
        })
        .await
        .map_err(|_| error())??;
        let granted = session::open(store.token()).await?;
        let mut sources = Vec::with_capacity(granted.monitors.len());
        let mut ids = BTreeMap::new();
        for monitor in &granted.monitors {
            let index = store.index(&monitor.stable_id)?;
            sources.push((index, monitor.node));
            ids.insert(index, monitor.mapping_id.clone());
        }
        // Start consumes the old token. Persist its replacement before native
        // setup can fail; the consent owner still closes every failure path.
        let token = granted.restore_token;
        let store = tokio::task::spawn_blocking(move || {
            store.save(token)?;
            Ok::<_, DesktopError>(store)
        })
        .await
        .map_err(|_| error())??;
        let mut capture = capture::Group::start(granted.video, sources)?;
        let slots = capture.slots.clone();
        let mappings = slots
            .iter()
            .map(|slot| {
                Ok((
                    slot.index,
                    (ids.remove(&slot.index).ok_or_else(error)?, slot.clone()),
                ))
            })
            .collect::<Result<_, DesktopError>>()?;
        let input = Arc::new(input::Group::start(granted.input, mappings)?);
        let closing_input = input.clone();
        let mut owner = granted.owner;
        let mut portal_closed = owner.closed.clone();
        let (close, stop) = oneshot::channel();
        let (closed_tx, closed) = watch::channel(false);
        let (finished_tx, finished) = watch::channel(false);
        tokio::spawn(async move {
            let _store = store;
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            tokio::pin!(stop);
            loop {
                tokio::select! {
                    _ = &mut stop => break,
                    changed = portal_closed.changed() => {
                        if changed.is_err() || *portal_closed.borrow() { break; }
                    },
                    _ = tick.tick() => {
                        if closing_input.stopped() || capture.slots.iter().any(|slot| slot.extent().is_err()) { break; }
                    },
                }
            }
            closed_tx.send_replace(true);
            capture.stop();
            closing_input.stop();
            let (video, input) = tokio::join!(capture.close(), closing_input.close());
            // Let our native peers drop before closing the portal normally.
            // A bounded join timeout still proceeds to portal revocation.
            owner.close().await;
            if video.is_err() || input.is_err() {
                tracing::warn!("Wayland native close did not complete cleanly");
            }
            // Completion is also the state-lock release boundary. A caller
            // may immediately restore a new session without retrying a lock.
            drop(_store);
            drop(capture);
            drop(closing_input);
            finished_tx.send_replace(true);
        });
        let backend = Self {
            slots,
            input,
            producers: Arc::new(Semaphore::new(8)),
            closed,
            close: Mutex::new(Some(close)),
            finished,
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if *backend.closed.borrow() {
                    return Err(error());
                }
                if backend
                    .input
                    .ready
                    .load(std::sync::atomic::Ordering::Acquire)
                    && backend
                        .slots
                        .iter()
                        .all(|slot| slot.extent().is_ok_and(|extent| extent.is_some()))
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| error())??;
        backend.capabilities()?;
        Ok(backend)
    }
    pub async fn close(&self) {
        self.request_close();
        let mut finished = self.finished.clone();
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                break;
            }
        }
    }
    fn request_close(&self) {
        if let Ok(mut close) = self.close.lock()
            && let Some(close) = close.take()
        {
            let _ = close.send(());
        }
    }
    fn slot(&self, display: u32) -> Result<Arc<capture::Slot>, DesktopError> {
        if *self.closed.borrow() {
            return Err(error());
        }
        self.slots
            .iter()
            .find(|slot| slot.index == display)
            .cloned()
            .ok_or_else(error)
    }
}
impl Drop for WaylandDesktop {
    fn drop(&mut self) {
        self.request_close();
    }
}
impl DesktopSource for WaylandDesktop {
    fn capabilities(&self) -> Result<DesktopCaps, DesktopError> {
        if *self.closed.borrow() {
            return Err(error());
        }
        let displays = self
            .slots
            .iter()
            .enumerate()
            .map(|(position, slot)| {
                let (width, height) = slot.extent()?.ok_or_else(error)?;
                Ok(DisplayInfo {
                    index: slot.index,
                    width,
                    height,
                    primary: position == 0,
                })
            })
            .collect::<Result<_, DesktopError>>()?;
        Ok(DesktopCaps {
            displays,
            codecs: vec![Codec::H264],
        })
    }
    fn producer(
        &self,
        display: u32,
        interval: Duration,
        height: Option<u32>,
        extent: Option<(u32, u32)>,
    ) -> Result<Box<dyn FrameProducer>, DesktopError> {
        let permit = self.producers.clone().try_acquire_owned().map_err(|_| {
            DesktopError::Capture("Wayland encoder session budget exhausted".into())
        })?;
        Ok(Box::new(producer::Producer::new(
            permit,
            self.slot(display)?,
            interval,
            height,
            extent,
        )?))
    }
    fn input(&self, display: u32, extent: (u32, u32)) -> Result<Box<dyn InputSink>, DesktopError> {
        if self.slot(display)?.extent()? != Some(extent) {
            return Err(error());
        }
        self.input.sink(display, extent)
    }
}
