use super::*;
use std::time::Instant;

struct Blocked {
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}

impl Write for Blocked {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.entered.try_send(());
        self.release.recv().unwrap();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn record() -> Buffer {
    let mut record = Buffer::default();
    record.write_all(b"fixture\n").unwrap();
    record
}

#[tokio::test]
async fn console_pause_stops_writes_bounds_backlog_and_resumes_on_drop() {
    #[derive(Clone, Default)]
    struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let capture = Capture::default();
    let output = Output::new(capture.clone(), 2).unwrap();
    output.sink().send(record());
    let pause = output.pause().await.unwrap();
    assert_eq!(*capture.0.lock().unwrap(), b"fixture\n");
    for _ in 0..5 {
        output.sink().send(record());
    }
    assert_eq!(output.health().telemetry_dropped_total, 3);
    assert_eq!(*capture.0.lock().unwrap(), b"fixture\n");
    drop(pause);
    assert!(output.shutdown().drained);
    assert_eq!(*capture.0.lock().unwrap(), b"fixture\nfixture\nfixture\n");
}

#[tokio::test]
async fn cancelled_pause_request_does_not_leave_output_paused() {
    let (entered, in_write) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(4);
    let output = Output::new(
        Blocked {
            entered,
            release: released,
        },
        4,
    )
    .unwrap();
    output.sink().send(record());
    in_write.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), output.pause())
            .await
            .is_err()
    );
    // The queued Pause's receiver is disconnected when the canceled future
    // drops its guard, so the worker must pass it after the blocked write ends.
    output.sink().send(record());
    release.send(()).unwrap();
    release.send(()).unwrap();
    assert!(output.shutdown().drained);
}

#[test]
fn blocked_writer_bounds_queue_and_shutdown_without_blocking_producers() {
    let (entered, in_write) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(4);
    let output = Output::new(
        Blocked {
            entered,
            release: released,
        },
        1,
    )
    .unwrap();
    let sink = output.sink();
    sink.send(record());
    in_write.recv_timeout(Duration::from_secs(2)).unwrap();
    sink.send(record()); // One queued record; the previous one is in write().
    let started = Instant::now();
    for _ in 0..10_000 {
        sink.send(record());
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(sink.health().telemetry_dropped_total, 10_000);
    let stopped = Instant::now();
    let report = output.shutdown();
    assert!(!report.drained);
    assert!(stopped.elapsed() < Duration::from_secs(2));
    sink.send(record());
    assert_eq!(sink.health().telemetry_dropped_total, 10_001);
    // Release the detached fixture worker; no permanently blocked test thread.
    release.send(()).unwrap();
    release.send(()).unwrap();
}

struct Broken;
impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}

#[test]
fn write_and_flush_errors_are_accounted_without_recursive_logging() {
    let output = Output::new(Broken, 4).unwrap();
    output.sink().send(record());
    let report = output.shutdown();
    assert!(report.drained);
    assert_eq!(report.health.telemetry_write_errors_total, 2);
    assert_eq!(report.health.telemetry_dropped_total, 0);
}

#[test]
fn buffer_rejects_excess_before_growing_and_never_keeps_partial_record() {
    let mut buffer = Buffer::default();
    assert!(
        buffer
            .write_all(&vec![b'x'; crate::MAX_RECORD_BYTES + 1])
            .is_err()
    );
    assert!(buffer.0.is_empty());
    assert_eq!(buffer.0.capacity(), 0);
    buffer
        .write_all(&vec![b'x'; crate::MAX_RECORD_BYTES])
        .unwrap();
    assert!(buffer.write_all(b"\n").is_err());
    assert_eq!(buffer.0.len(), crate::MAX_RECORD_BYTES);
    assert!(buffer.0.capacity() <= crate::MAX_RECORD_BYTES);
}

#[test]
fn blocking_writer_destructor_does_not_escape_the_shutdown_deadline() {
    struct BlockDrop(mpsc::Receiver<()>);
    impl Write for BlockDrop {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Drop for BlockDrop {
        fn drop(&mut self) {
            let _ = self.0.recv();
        }
    }
    let (release, blocked) = mpsc::sync_channel(1);
    let output = Output::new(BlockDrop(blocked), 1).unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    let waiter = std::thread::spawn(move || send.send(output.shutdown()).unwrap());
    let result = receive.recv_timeout(Duration::from_secs(2));
    // Always unblock the fixture before asserting, including on a regression.
    release.send(()).unwrap();
    waiter.join().unwrap();
    assert!(
        !result
            .expect("shutdown waited for a blocking destructor")
            .drained
    );
}

#[test]
fn racing_shutdown_accounts_for_every_producer_record() {
    struct Counted(Arc<AtomicU64>);
    impl Write for Counted {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for _ in 0..32 {
        let written = Arc::new(AtomicU64::new(0));
        let output = Output::new(Counted(written.clone()), 4).unwrap();
        let sink = output.sink();
        let start = std::sync::Barrier::new(5);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let sink = &sink;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..100 {
                        sink.send(record());
                    }
                });
            }
            start.wait();
            assert!(output.shutdown().drained);
        });
        assert_eq!(
            written.load(Ordering::Relaxed) + sink.health().telemetry_dropped_total,
            400
        );
    }
}
