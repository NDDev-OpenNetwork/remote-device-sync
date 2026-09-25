//! Blocking stderr is isolated in one dedicated output adapter thread, never
//! Tokio's blocking pool (which would make runtime shutdown wait indefinitely).
//! No transport, media or service work runs here. A stuck OS write cannot be
//! cancelled safely: bounded shutdown reports failure and detaches this one
//! thread, which process exit reclaims. It never prints a timeout to stdout.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;

const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Health {
    pub telemetry_dropped_total: u64,
    pub telemetry_oversize_total: u64,
    pub telemetry_write_errors_total: u64,
}

#[derive(Default)]
struct Counters {
    dropped: AtomicU64,
    oversize: AtomicU64,
    write_errors: AtomicU64,
    closed: AtomicBool,
}

impl Counters {
    fn health(&self) -> Health {
        Health {
            telemetry_dropped_total: self.dropped.load(Ordering::Relaxed),
            telemetry_oversize_total: self.oversize.load(Ordering::Relaxed),
            telemetry_write_errors_total: self.write_errors.load(Ordering::Relaxed),
        }
    }
}

// Every formatting/serialization write checks the budget before allocation.
#[derive(Default)]
pub(super) struct Buffer(Vec<u8>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > crate::MAX_RECORD_BYTES - self.0.len() {
            return Err(io::Error::other("log record exceeds byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl std::fmt::Write for Buffer {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| std::fmt::Error)
    }
}

#[derive(Clone)]
pub(super) struct Sink {
    sender: tokio::sync::mpsc::Sender<Message>,
    counters: Arc<Counters>,
}

enum Message {
    Record(Buffer),
    Wake,
}

impl Sink {
    pub fn health(&self) -> Health {
        self.counters.health()
    }
    pub fn oversize(&self) {
        self.counters.oversize.fetch_add(1, Ordering::Relaxed);
    }
    pub fn send(&self, buffer: Buffer) {
        if self.counters.closed.load(Ordering::Acquire)
            || self.sender.try_send(Message::Record(buffer)).is_err()
        {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// `drained` means the output worker finished. Check write_errors separately;
/// this is never a receipt from Vector or OpenObserve.
#[derive(Clone, Copy, Debug)]
pub struct Shutdown {
    pub drained: bool,
    pub health: Health,
}

pub(super) struct Output {
    sink: Sink,
    done: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
}

impl Output {
    pub fn new<W: Write + Send + 'static>(mut writer: W, capacity: usize) -> io::Result<Self> {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Message>(capacity);
        let (done_sender, done) = mpsc::sync_channel(1);
        let counters = Arc::new(Counters::default());
        let worker_counters = counters.clone();
        let worker = std::thread::Builder::new()
            .name("rds-log-output".into())
            .spawn(move || {
                loop {
                    if worker_counters.closed.load(Ordering::Acquire) {
                        // Close admission atomically, then drain every accepted
                        // record. A racing producer is either drained or rejected.
                        receiver.close();
                    }
                    let record = match receiver.blocking_recv() {
                        Some(Message::Record(record)) => record,
                        Some(Message::Wake) => continue,
                        None => break,
                    };
                    if writer.write_all(&record.0).is_err() {
                        worker_counters.write_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if writer.flush().is_err() {
                    worker_counters.write_errors.fetch_add(1, Ordering::Relaxed);
                }
                // Destructors can block too; finish the writer before signaling
                // completion to the thread that may synchronously join us.
                drop(writer);
                let _ = done_sender.send(());
            })?;
        Ok(Self {
            sink: Sink { sender, counters },
            done,
            worker: Some(worker),
        })
    }

    pub fn sink(&self) -> Sink {
        self.sink.clone()
    }
    pub fn health(&self) -> Health {
        self.sink.health()
    }

    pub fn shutdown(mut self) -> Shutdown {
        self.close()
    }

    fn close(&mut self) -> Shutdown {
        self.sink.counters.closed.store(true, Ordering::Release);
        // Wake an idle worker without periodic polling. A full queue already
        // wakes it; a blocked OS writer still obeys our bounded completion wait.
        let _ = self.sink.sender.try_send(Message::Wake);
        let drained = self.done.recv_timeout(DRAIN_TIMEOUT).is_ok();
        // Otherwise dropping JoinHandle detaches the potentially blocked writer.
        if let Some(worker) = self.worker.take()
            && drained
        {
            let joined = worker.join().is_ok();
            return Shutdown {
                drained: joined,
                health: self.health(),
            };
        }
        Shutdown {
            drained,
            health: self.health(),
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests;
