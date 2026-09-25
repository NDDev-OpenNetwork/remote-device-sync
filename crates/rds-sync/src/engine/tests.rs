use super::*;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("rds-sink-cancel-{:032x}", rand::random::<u128>())))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn queued_writer(cancel_finish: bool) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let root = Scratch::new();
    let data = vec![17u8; 4096];
    let manifest = crate::manifest_of(&data);
    runtime.block_on(async {
        // Occupy the sole blocking worker. The queued store cannot have started
        // when the async receiver/finish future is canceled.
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            blocked.recv().unwrap();
        });
        ready.await.unwrap();
        let journal = Journal::open(&root.0, "data.bin", &manifest).unwrap();
        let (sink, stopped) = JournalSink::start(journal);
        sink.put(0, data).await.unwrap();
        if cancel_finish {
            assert!(
                tokio::time::timeout(Duration::from_millis(10), sink.finish())
                    .await
                    .is_err()
            );
        } else {
            drop(sink);
        }
        release.send(()).unwrap();
        blocker.await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), stopped)
            .await
            .unwrap();
        // The same blocking worker must have returned and dropped its output.
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        let resumed = Journal::open(&root.0, "data.bin", &manifest).unwrap();
        assert!(
            resumed.have_set().is_empty(),
            "canceled queued chunk was still written to disk"
        );
    });
}

#[test]
fn dropping_sink_discards_stores_that_have_not_started() {
    queued_writer(false);
}

#[test]
fn canceling_finish_discards_stores_that_have_not_started() {
    queued_writer(true);
}

#[tokio::test]
async fn normal_finish_drains_and_verifies_every_queued_store() {
    let root = Scratch::new();
    let data = vec![19u8; 4096];
    let manifest = crate::manifest_of(&data);
    let journal = Journal::open(&root.0, "data.bin", &manifest).unwrap();
    let (sink, _stopped) = JournalSink::start(journal);
    sink.put(0, data.clone()).await.unwrap();
    let journal = sink.finish().await.unwrap();
    assert!(journal.complete());
    let path = journal.assemble().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), data);
}
