//! Bounded task ownership shared by the runner and shutdown fallback.
//!
//! Admission holds the same short lock as sealing. Only the runner, or a
//! serialized close waiter after the runner exits, polls completion. This
//! keeps both active tasks and completed handles within the configured budget.
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, oneshot};
use tokio::task::JoinSet;

use super::{State, route};
use crate::http::{Request, Response};

pub(super) struct TaskGroup {
    inner: Mutex<Inner>,
    changed: Notify,
    limit: usize,
}
struct Inner {
    sealed: bool,
    tasks: JoinSet<()>,
}
impl TaskGroup {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                sealed: false,
                tasks: JoinSet::new(),
            }),
            changed: Notify::new(),
            limit,
        }
    }
    pub(super) fn request(
        &self,
        state: Arc<State>,
        peer: SocketAddr,
        req: Request,
    ) -> Option<oneshot::Receiver<Response>> {
        let (tx, rx) = oneshot::channel();
        self.spawn_job(move || {
            let response = route(&state, peer, &req);
            let _ = tx.send(response);
        })
        .then_some(rx)
    }
    pub(super) fn collect(&self, state: Arc<State>) {
        let _ = self.spawn_job(move || match state.store.collect_expired() {
            Ok(count) => {
                state
                    .metrics
                    .gc_retired
                    .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
            }
            Err(_) => {
                state
                    .metrics
                    .gc_failures
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    fn admit(&self) -> Option<std::sync::MutexGuard<'_, Inner>> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        while inner.tasks.try_join_next().is_some() {}
        if inner.sealed || inner.tasks.len() >= self.limit {
            return None;
        }
        Some(inner)
    }
    fn spawn_job(&self, job: impl FnOnce() + Send + 'static) -> bool {
        let Some(mut inner) = self.admit() else {
            return false;
        };
        inner.tasks.spawn_blocking(job);
        self.changed.notify_one();
        true
    }
    pub(super) fn spawn(&self, job: impl Future<Output = ()> + Send + 'static) -> bool {
        let Some(mut inner) = self.admit() else {
            return false;
        };
        inner.tasks.spawn(job);
        self.changed.notify_one();
        true
    }
    pub(super) fn seal(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.sealed = true;
        // Only blocking jobs that have not started can be canceled. Already
        // running filesystem operations remain charged and must be joined.
        inner.tasks.abort_all();
        self.changed.notify_one();
    }
    pub(super) fn snapshot(&self) -> Option<(usize, usize)> {
        let inner = self.inner.try_lock().ok()?;
        Some((inner.tasks.len(), self.limit))
    }
    pub(super) fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tasks
            .is_empty()
    }
    pub(super) async fn changed(&self) {
        self.changed.notified().await;
    }
    pub(super) async fn join_next(&self) -> Option<Result<(), tokio::task::JoinError>> {
        poll_fn(|cx| {
            self.inner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .tasks
                .poll_join_next(cx)
        })
        .await
    }
    pub(super) async fn drain(&self) {
        self.seal();
        while self.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Directory, Runner};
    use super::*;
    use std::time::Duration;
    use tokio::sync::watch;

    #[tokio::test]
    async fn observer_cancellation_preserves_live_directory_and_concurrent_close() {
        let directory = super::super::serve(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(crate::MemoryStore::default()),
            super::super::ServiceConfig::default(),
        )
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), directory.wait_stopped())
                .await
                .is_err()
        );
        crate::client::Client::new(directory.addr())
            .health()
            .await
            .unwrap();
        let (observed, closed) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(directory.wait_stopped(), directory.close())
        })
        .await
        .unwrap();
        observed.unwrap();
        closed.unwrap();
        directory.wait_stopped().await.unwrap();
        directory.close().await.unwrap();
    }

    struct Release(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    async fn block(workers: &TaskGroup) -> Release {
        let (tx, rx) = std::sync::mpsc::channel();
        let (started, ready) = oneshot::channel();
        assert!(workers.spawn_job(move || {
            let _ = started.send(());
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
        tokio::time::timeout(Duration::from_secs(2), ready)
            .await
            .unwrap()
            .unwrap();
        Release(Some(tx))
    }

    #[tokio::test]
    async fn canceled_fallback_join_retains_runner_failure_and_all_task_groups() {
        let connections = Arc::new(TaskGroup::new(1));
        let (entered, ready) = oneshot::channel();
        let (destroyed, dropped) = oneshot::channel();
        struct NotifyDrop(Option<oneshot::Sender<()>>);
        impl Drop for NotifyDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        assert!(connections.spawn(async move {
            let _guard = NotifyDrop(Some(destroyed));
            let _ = entered.send(());
            std::future::pending::<()>().await;
        }));
        tokio::time::timeout(Duration::from_secs(2), ready)
            .await
            .unwrap()
            .unwrap();
        let workers = Arc::new(TaskGroup::new(1));
        let maintenance = Arc::new(TaskGroup::new(1));
        let request_release = block(&workers).await;
        let gc_release = block(&maintenance).await;
        let (shutdown, _) = watch::channel(false);
        let directory = Directory {
            records: None,
            policy: None,
            metrics: Default::default(),
            addr: "127.0.0.1:0".parse().unwrap(),
            shutdown,
            connections: connections.clone(),
            workers: workers.clone(),
            maintenance: maintenance.clone(),
            runner: tokio::sync::Mutex::new(Runner {
                task: Some(tokio::spawn(async { panic!("fixture runner failure") })),
                outcome: None,
            }),
        };
        // Observe runner failure before the controlled blocking jobs finish.
        // A host can now initiate shutdown of its other service immediately.
        for _ in 0..2 {
            let observed = tokio::time::timeout(Duration::from_secs(2), directory.wait_stopped())
                .await
                .unwrap()
                .unwrap_err();
            assert!(observed.to_string().contains("fixture runner failure"));
        }
        assert!(!workers.is_empty());
        assert!(!maintenance.is_empty());
        // The runner has failed, so this waiter is canceled in fallback drain.
        assert!(
            tokio::time::timeout(Duration::from_millis(75), directory.close())
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(2), dropped)
            .await
            .unwrap()
            .unwrap();
        assert!(!connections.spawn(async { panic!("sealed connection admitted") }));
        assert!(!workers.spawn_job(|| panic!("sealed request admitted")));
        assert!(!maintenance.spawn_job(|| panic!("sealed maintenance admitted")));
        drop(request_release);
        assert!(
            tokio::time::timeout(Duration::from_millis(75), directory.close())
                .await
                .is_err()
        );
        drop(gc_release);
        for _ in 0..2 {
            let error = tokio::time::timeout(Duration::from_secs(2), directory.close())
                .await
                .unwrap()
                .unwrap_err();
            assert!(error.to_string().contains("fixture runner failure"));
        }
        assert!(connections.is_empty());
        assert!(workers.is_empty());
        assert!(maintenance.is_empty());
    }

    #[test]
    fn shutdown_cancels_a_queued_blocking_job_without_executing_it() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let workers = TaskGroup::new(2);
            let release = block(&workers).await;
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = calls.clone();
            assert!(workers.spawn_job(move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
            assert!(!workers.spawn_job(|| panic!("over-budget work admitted")));
            workers.seal();
            drop(release);
            tokio::time::timeout(Duration::from_secs(2), workers.drain())
                .await
                .unwrap();
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert!(workers.is_empty());
        });
    }
}
