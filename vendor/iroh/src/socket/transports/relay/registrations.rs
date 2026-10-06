//! Bounded, opt-in readiness of independently registered relay connections.

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use iroh_base::RelayUrl;
use iroh_relay::RelayMap;
use n0_watcher::Watchable;

/// Initial configured URLs are the only persistent registrations. Dynamic
/// additions do not expand this budget; removal immediately withdraws readiness.
#[derive(Debug, Clone)]
pub(crate) struct RegisteredRelays {
    urls: Arc<BTreeSet<RelayUrl>>,
    relay_map: RelayMap,
    ready: Watchable<BTreeSet<RelayUrl>>,
    writers: Arc<Mutex<()>>,
    stopped: Arc<AtomicBool>,
}

impl RegisteredRelays {
    pub(crate) fn new(enabled: bool, relay_map: RelayMap) -> Self {
        Self {
            urls: Arc::new(if enabled {
                relay_map.urls()
            } else {
                BTreeSet::new()
            }),
            relay_map,
            ready: Watchable::new(BTreeSet::new()),
            writers: Arc::new(Mutex::new(())),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        !self.urls.is_empty()
    }

    pub(crate) fn persistent(&self, url: &RelayUrl) -> bool {
        self.urls.contains(url) && self.relay_map.contains(url)
    }

    pub(crate) fn configured(&self) -> Vec<RelayUrl> {
        self.urls
            .iter()
            .filter(|url| self.relay_map.contains(url))
            .cloned()
            .collect()
    }

    pub(crate) fn set_connected(&self, url: &RelayUrl, connected: bool) {
        let _guard = self.writers.lock().unwrap_or_else(|err| err.into_inner());
        let mut ready = self.ready.get();
        if connected && self.persistent(url) && !self.stopped.load(Ordering::Acquire) {
            ready.insert(url.clone());
        } else {
            ready.remove(url);
        }
        ready.retain(|url| !self.stopped.load(Ordering::Acquire) && self.persistent(url));
        let ready_count = ready.len();
        if self.ready.set(ready).is_ok() {
            tracing::info!(ready_count, "persistent relay readiness changed");
        }
    }

    pub(crate) fn refresh(&self) {
        let _guard = self.writers.lock().unwrap_or_else(|err| err.into_inner());
        let mut ready = self.ready.get();
        ready.retain(|url| !self.stopped.load(Ordering::Acquire) && self.persistent(url));
        let _ = self.ready.set(ready);
    }

    /// Seal publication synchronously before endpoint shutdown can return.
    /// Transport connections still drain through their existing task owners.
    pub(crate) fn stop(&self) {
        let _guard = self.writers.lock().unwrap_or_else(|err| err.into_inner());
        self.stopped.store(true, Ordering::Release);
        if self.ready.set(BTreeSet::new()).is_ok() {
            tracing::info!(
                ready_count = 0,
                "persistent relay publication sealed for shutdown"
            );
        }
    }

    pub(crate) fn watch(&self) -> n0_watcher::Direct<BTreeSet<RelayUrl>> {
        self.ready.watch()
    }

    pub(crate) fn lease(&self, url: RelayUrl) -> RegistrationLease {
        RegistrationLease {
            registered: self.clone(),
            url,
        }
    }
}

/// Readiness cannot survive a task exit, abort or panic.
pub(crate) struct RegistrationLease {
    registered: RegisteredRelays,
    url: RelayUrl,
}

impl Drop for RegistrationLease {
    fn drop(&mut self) {
        self.registered.set_connected(&self.url, false);
    }
}
