//! Bounded retry ownership for private native incident files. This worker is
//! independent of input/media; failure is observable without dumping contents.
use std::{collections::VecDeque, io, path::PathBuf};

const MAX_PENDING: usize = 4;
const MAX_INCIDENT_BYTES: usize = 256 * 1024;

#[derive(Clone, Default, serde::Serialize)]
pub(super) struct Health {
    pub enabled: bool,
    pub snapshot_writes_total: u64,
    pub snapshot_write_errors_total: u64,
    pub incident_writes_total: u64,
    pub incident_write_errors_total: u64,
    pub incident_queue_evicted_total: u64,
    pub incident_oversize_total: u64,
    pub incident_queue_pending: usize,
    pub last_snapshot_success_elapsed_ms: Option<u64>,
    pub last_incident_success_elapsed_ms: Option<u64>,
    pub last_error_kind: Option<&'static str>,
}

pub(super) struct Storage {
    directory: Option<PathBuf>,
    pending: VecDeque<Vec<u8>>,
    health: Health,
    snapshot_name: String,
}

impl Storage {
    pub(super) fn new(directory: Option<PathBuf>) -> Self {
        Self {
            health: Health {
                enabled: directory.is_some(),
                ..Health::default()
            },
            directory,
            pending: VecDeque::new(),
            snapshot_name: format!("state-{}.json", std::process::id()),
        }
    }

    pub(super) fn scoped(directory: Option<PathBuf>, tab: u64, revision: u64) -> Self {
        let mut storage = Self::new(directory);
        storage.snapshot_name = format!("state-{}-tab-{tab}-{revision}.json", std::process::id());
        storage
    }

    pub(super) async fn remove_snapshot(&self) {
        let Some(directory) = &self.directory else {
            return;
        };
        let path = directory.join(&self.snapshot_name);
        let _ = tokio::task::spawn_blocking(move || -> io::Result<()> {
            use std::os::unix::fs::MetadataExt;
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            if metadata.is_file()
                && metadata.nlink() == 1
                && metadata.mode() & 0o777 == 0o600
                && metadata.uid() == rustix::process::geteuid().as_raw()
            {
                std::fs::remove_file(path)?;
            }
            Ok(())
        })
        .await;
    }

    pub(super) fn health(&self) -> Health {
        self.health.clone()
    }

    fn enqueue(&mut self, incident: Option<Vec<u8>>) {
        let Some(bytes) = incident.filter(|_| self.directory.is_some()) else {
            return;
        };
        if bytes.len() > MAX_INCIDENT_BYTES {
            self.health.incident_oversize_total += 1;
            return;
        }
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
            self.health.incident_queue_evicted_total += 1;
        }
        self.pending.push_back(bytes);
        self.health.incident_queue_pending = self.pending.len();
    }

    fn complete_incident(&mut self, result: io::Result<()>, elapsed_ms: u64) {
        match result {
            Ok(()) => {
                self.pending.pop_front();
                self.health.incident_writes_total += 1;
                self.health.last_incident_success_elapsed_ms = Some(elapsed_ms);
            }
            Err(error) => {
                self.health.incident_write_errors_total += 1;
                self.health.last_error_kind = Some(error_kind(&error));
                tracing::warn!(
                    error_kind = error_kind(&error),
                    pending = self.pending.len(),
                    "viewer incident retained for retry after storage failure"
                );
            }
        }
        self.health.incident_queue_pending = self.pending.len();
    }

    pub(super) async fn persist(
        &mut self,
        snapshot: Option<Vec<u8>>,
        incident: Option<Vec<u8>>,
        elapsed_ms: u64,
    ) {
        self.enqueue(incident);
        let Some(directory) = self.directory.clone() else {
            return;
        };
        if let Some(bytes) = snapshot {
            let path = directory.join(&self.snapshot_name);
            let result =
                tokio::task::spawn_blocking(move || crate::logging::viewer_snapshot(&path, &bytes))
                    .await;
            match result.unwrap_or_else(|_| Err(io::Error::other("snapshot worker ended"))) {
                Ok(()) => {
                    self.health.snapshot_writes_total += 1;
                    self.health.last_snapshot_success_elapsed_ms = Some(elapsed_ms);
                }
                Err(error) => {
                    self.health.snapshot_write_errors_total += 1;
                    self.health.last_error_kind = Some(error_kind(&error));
                    tracing::warn!(
                        error_kind = error_kind(&error),
                        "viewer snapshot write failed"
                    );
                }
            }
        }
        // One bounded attempt per diagnostic tick. A failed window remains
        // owned; the next tick retries rather than silently discarding it.
        if let Some(bytes) = self.pending.front().cloned() {
            let result = tokio::task::spawn_blocking(move || {
                crate::logging::viewer_incident(&directory, &bytes)
            })
            .await;
            self.complete_incident(
                result.unwrap_or_else(|_| Err(io::Error::other("incident worker ended"))),
                elapsed_ms,
            );
        }
    }
}

fn error_kind(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::StorageFull => "storage_full",
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::AlreadyExists => "already_exists",
        _ => "io_failure",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_failure_retains_the_exact_incident_until_a_successful_retry() {
        let mut storage = Storage::new(Some(PathBuf::from("/unused")));
        storage.enqueue(Some(b"bounded synthetic evidence".to_vec()));
        storage.complete_incident(Err(io::Error::from_raw_os_error(28)), 2000);
        assert_eq!(
            storage.pending.front().unwrap(),
            b"bounded synthetic evidence"
        );
        assert_eq!(storage.health().incident_queue_pending, 1);
        assert_eq!(storage.health().incident_write_errors_total, 1);
        assert_eq!(storage.health().last_error_kind, Some("storage_full"));
        assert_eq!(storage.health().incident_writes_total, 0);
        storage.complete_incident(Ok(()), 4000);
        assert!(storage.pending.is_empty());
        assert_eq!(storage.health().incident_writes_total, 1);
        assert_eq!(
            storage.health().last_incident_success_elapsed_ms,
            Some(4000)
        );
    }

    #[test]
    fn outage_backlog_is_bounded_and_evictions_are_explicit() {
        let mut storage = Storage::new(Some(PathBuf::from("/unused")));
        for n in 0..6 {
            storage.enqueue(Some(vec![n; MAX_INCIDENT_BYTES]));
        }
        assert_eq!(storage.pending.len(), 4);
        assert_eq!(storage.pending.front().unwrap()[0], 2);
        assert_eq!(storage.health().incident_queue_evicted_total, 2);
        storage.enqueue(Some(vec![0; MAX_INCIDENT_BYTES + 1]));
        assert_eq!(storage.health().incident_oversize_total, 1);
        assert_eq!(storage.pending.len(), 4);
        let mut disabled = Storage::new(None);
        disabled.enqueue(Some(vec![0; 16]));
        assert!(disabled.pending.is_empty());
        assert!(!disabled.health().enabled);
    }
}
