//! Passive reply-read progress. The observer never polls or cancels the reader.
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, ReadBuf};
use tracing::Instrument;

const SLOW: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Default)]
pub(crate) struct PendingReplies {
    pub count: usize,
    pub oldest_seq: Option<u64>,
    pub oldest_age: Option<Duration>,
}

#[derive(Default)]
struct State {
    generation: u64,
    active: bool,
    started: Option<Instant>,
    last_progress: Option<Instant>,
    polls: u64,
    inside_poll: bool,
    bytes: usize,
    prefix: [u8; 4],
}

#[derive(Clone, Copy)]
struct Snapshot {
    generation: u64,
    active: bool,
    elapsed: Duration,
    progress_age: Duration,
    polls: u64,
    inside_poll: bool,
    bytes: usize,
    expected_body: Option<u32>,
}

#[derive(Clone, Default)]
pub(crate) struct ReadProgress(Arc<Mutex<State>>);

impl ReadProgress {
    pub fn begin(&self) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let generation = state.generation.saturating_add(1);
        *state = State {
            generation,
            active: true,
            started: Some(Instant::now()),
            ..State::default()
        };
    }

    pub fn end(&self) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active = false;
    }

    fn snapshot(&self) -> Option<Snapshot> {
        // The observer cannot make the reader wait for a diagnostic query.
        let state = self.0.try_lock().ok()?;
        let started = state.started?;
        Some(Snapshot {
            generation: state.generation,
            active: state.active,
            elapsed: started.elapsed(),
            progress_age: state.last_progress.unwrap_or(started).elapsed(),
            polls: state.polls,
            inside_poll: state.inside_poll,
            bytes: state.bytes,
            expected_body: (state.bytes >= 4).then(|| u32::from_be_bytes(state.prefix)),
        })
    }

    pub fn observe(
        &self,
        instance: u64,
        pending: impl Fn() -> Option<PendingReplies> + Send + 'static,
    ) -> ReadObserver {
        let weak = Arc::downgrade(&self.0);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(SLOW);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.reset();
            let mut last = None::<(u64, Instant)>;
            loop {
                let scheduled = tick.tick().await;
                let Some(state) = weak.upgrade() else { break };
                let Some(read) = ReadProgress(state).snapshot() else {
                    continue;
                };
                let probes = pending();
                let overdue = probes
                    .and_then(|p| p.oldest_age)
                    .is_some_and(|age| age >= SLOW);
                let partial = read.bytes > 0 && read.progress_age >= SLOW;
                let late = scheduled.elapsed() >= SLOW;
                if (!read.active || (!overdue && !partial)) && !late {
                    continue;
                }
                if last.is_some_and(|(generation, at)| {
                    generation == read.generation && at.elapsed() < Duration::from_secs(1)
                }) {
                    continue;
                }
                last = Some((read.generation, Instant::now()));
                tracing::warn!(target:"rds_desktop::control_timing",
                    control_instance=instance, read_generation=read.generation,
                    read_elapsed_ms=read.elapsed.as_millis(),
                    read_progress_age_ms=read.progress_age.as_millis(),
                    read_poll_calls=read.polls, inside_poll_read=read.inside_poll,
                    frame_bytes_read=read.bytes, prefix_bytes_read=read.bytes.min(4),
                    expected_body_bytes=read.expected_body, body_bytes_read=read.bytes.saturating_sub(4),
                    probe_snapshot_busy=probes.is_none(), pending_replies=probes.map(|p|p.count),
                    oldest_pending_seq=probes.and_then(|p|p.oldest_seq),
                    oldest_pending_age_ms=probes.and_then(|p|p.oldest_age).map(|age|age.as_millis()),
                    observer_tick_late_ms=scheduled.elapsed().as_millis(),
                    "desktop reply read awaiting progress; passive observation");
            }
        }.in_current_span());
        ReadObserver(task)
    }
}

pub(crate) struct ReadObserver(tokio::task::JoinHandle<()>);

impl Drop for ReadObserver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct ObservedRead<R> {
    reader: R,
    progress: ReadProgress,
}

impl<R> ObservedRead<R> {
    pub fn new(reader: R, progress: ReadProgress) -> Self {
        Self { reader, progress }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ObservedRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        {
            let mut state = self
                .progress
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.polls = state.polls.saturating_add(1);
            state.inside_poll = true;
        }
        let before = buf.filled().len();
        // Never retain the progress mutex while calling the actual I/O driver.
        let result = Pin::new(&mut self.reader).poll_read(cx, buf);
        let mut state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.inside_poll = false;
        let bytes = &buf.filled()[before..];
        if !bytes.is_empty() {
            let prefix_count = state.bytes.min(4);
            let copied = (4 - prefix_count).min(bytes.len());
            state.prefix[prefix_count..prefix_count + copied].copy_from_slice(&bytes[..copied]);
            state.bytes = state.bytes.saturating_add(bytes.len());
            state.last_progress = Some(Instant::now());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn until_bytes(progress: &ReadProgress, bytes: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while progress
                .snapshot()
                .is_none_or(|snapshot| snapshot.bytes != bytes)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn partial_prefix_and_body_remain_one_uncancelled_framed_read() {
        let event = rds_core::DesktopEvent::Heartbeat { seq: 7, ts_ms: 11 };
        let mut wire = Vec::new();
        rds_net::write_frame(&mut wire, &event).await.unwrap();
        let (mut writer, reader) = tokio::io::duplex(128);
        let progress = ReadProgress::default();
        let shared = progress.clone();
        let _monitor = progress.observe(1, || {
            Some(PendingReplies {
                count: 1,
                oldest_seq: Some(7),
                oldest_age: Some(Duration::from_secs(1)),
            })
        });
        let read = tokio::spawn(async move {
            let mut reader = ObservedRead::new(reader, shared.clone());
            shared.begin();
            let result = rds_net::read_frame::<_, rds_core::DesktopEvent>(&mut reader).await;
            shared.end();
            result
        });
        writer.write_all(&wire[..2]).await.unwrap();
        until_bytes(&progress, 2).await;
        assert_eq!(progress.snapshot().unwrap().expected_body, None);
        // Let observation tick while an actual partial framed read is pending.
        tokio::time::sleep(Duration::from_millis(300)).await;
        writer.write_all(&wire[2..5]).await.unwrap();
        until_bytes(&progress, 5).await;
        let snapshot = progress.snapshot().unwrap();
        assert_eq!(snapshot.expected_body, Some((wire.len() - 4) as u32));
        writer.write_all(&wire[5..]).await.unwrap();
        match tokio::time::timeout(Duration::from_secs(1), read)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            rds_core::DesktopEvent::Heartbeat { seq: 7, ts_ms: 11 } => {}
            other => panic!("framed reply changed: {other:?}"),
        }
        let snapshot = progress.snapshot().unwrap();
        assert!(!snapshot.active);
        assert_eq!(snapshot.bytes, wire.len());
    }

    struct NeverWakes(Arc<AtomicUsize>);
    impl AsyncRead for NeverWakes {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Poll::Pending
        }
    }

    #[tokio::test]
    async fn observer_neither_repolls_a_blocked_reader_nor_outlives_its_owner() {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(AtomicUsize::new(0));
        let progress = ReadProgress::default();
        let count = observed.clone();
        let monitor = progress.observe(2, move || {
            count.fetch_add(1, Ordering::Relaxed);
            Some(PendingReplies {
                count: 1,
                oldest_seq: Some(1),
                oldest_age: Some(Duration::from_secs(1)),
            })
        });
        let shared = progress.clone();
        let calls = polls.clone();
        let read = tokio::spawn(async move {
            shared.begin();
            let mut reader = ObservedRead::new(NeverWakes(calls), shared);
            reader.read_exact(&mut [0u8; 1]).await
        });
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(observed.load(Ordering::Relaxed) >= 2);
        assert_eq!(
            polls.load(Ordering::Relaxed),
            1,
            "observer retried the I/O future and could mask a missing wake"
        );
        assert_eq!(progress.snapshot().unwrap().polls, 1);
        assert!(!progress.snapshot().unwrap().inside_poll);
        drop(monitor);
        tokio::task::yield_now().await;
        let stopped = observed.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(observed.load(Ordering::Relaxed), stopped);
        read.abort();
        assert!(read.await.unwrap_err().is_cancelled());
    }
}
