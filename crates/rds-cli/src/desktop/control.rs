//! Managed native control dispatch progresses independently of decode waits.
use rds_core::DesktopControl;
use rds_desktop::render::{InputReceiver, ViewerInput};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub(super) trait Input {
    fn recv(&mut self) -> impl Future<Output = Option<ViewerInput>> + Send;
}
impl Input for InputReceiver {
    async fn recv(&mut self) -> Option<ViewerInput> {
        self.recv().await
    }
}
pub(super) trait Sender {
    fn send(&self, message: DesktopControl) -> impl Future<Output = anyhow::Result<()>> + Send;
}
impl Sender for rds_client::local::ManagedControl {
    async fn send(&self, message: DesktopControl) -> anyhow::Result<()> {
        Ok(self.control(message).await?)
    }
}

pub(super) async fn pump(
    input: &mut impl Input,
    sender: &impl Sender,
    progress: &watch::Receiver<tokio::time::Instant>,
    started: Instant,
    sent: impl Fn(&DesktopControl),
) -> anyhow::Result<bool> {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let message = tokio::select! {
            message = input.recv() => match message {
                Some(ViewerInput::Control(message)) => message,
                Some(ViewerInput::Close) | None => return Ok(true),
            },
            _ = tick.tick() => {
                let age = progress.borrow().elapsed();
                if age >= Duration::from_secs(15) {
                    tracing::warn!(last_decoded_age_ms = age.as_millis() as u64,
                        "desktop progress watchdog expired");
                    anyhow::bail!("remote video stopped making progress");
                }
                DesktopControl::Heartbeat {
                    seq: 0, ts_ms: started.elapsed().as_millis() as u64,
                }
            },
        };
        sent(&message);
        // A partially written control is never reused after timeout: the
        // owner ends both legs and drops the channel before reconnecting.
        tokio::time::timeout(Duration::from_secs(2), sender.send(message))
            .await
            .map_err(|_| anyhow::anyhow!("desktop control write stalled"))??;
    }
}

pub(super) async fn run(
    controls: impl Future<Output = anyhow::Result<bool>>,
    media: impl Future<Output = anyhow::Result<bool>>,
    stop: &CancellationToken,
) -> anyhow::Result<bool> {
    // Neither leg is respawned or reconstructed after the other wakes. On
    // completion all pending async work is dropped with the owning session.
    tokio::select! {
        biased;
        _ = stop.cancelled() => Ok(true),
        result = controls => result,
        result = media => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rds_core::{InputEvent, InputKind, local::DesktopUp};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::{
        io::DuplexStream,
        sync::{Mutex, mpsc, oneshot},
    };

    impl Input for mpsc::Receiver<ViewerInput> {
        async fn recv(&mut self) -> Option<ViewerInput> {
            self.recv().await
        }
    }
    struct Wire(Mutex<DuplexStream>);
    impl Sender for Wire {
        async fn send(&self, message: DesktopControl) -> anyhow::Result<()> {
            Ok(
                rds_net::write_frame(&mut *self.0.lock().await, &DesktopUp::Control(message))
                    .await?,
            )
        }
    }
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    // Read the actual framed controls, rather than only observing that an
    // input was dequeued. The media leg remains held until the session ends.
    #[tokio::test(start_paused = true)]
    async fn blocked_decode_does_not_hold_input_or_heartbeat_and_close_drops_it() {
        let (wire, mut remote) = tokio::io::duplex(256);
        let (tx, mut input) = mpsc::channel(8);
        let stop = CancellationToken::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let held = dropped.clone();
        let (decoding, started_decode) = oneshot::channel();
        let (_release, blocked) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let (_progress, last_frame) = watch::channel(tokio::time::Instant::now());
            let media = async {
                let _drop = Dropped(held);
                decoding.send(()).unwrap();
                blocked.await.unwrap();
                Ok(false)
            };
            run(
                pump(
                    &mut input,
                    &Wire(Mutex::new(wire)),
                    &last_frame,
                    Instant::now(),
                    |_| {},
                ),
                media,
                &stop,
            )
            .await
        });
        started_decode.await.unwrap();
        let started = tokio::time::Instant::now();
        for (seq, kind) in [
            InputKind::KeyDown { code: 56 },
            InputKind::KeyDown { code: 105 },
            InputKind::KeyUp { code: 105 },
            InputKind::KeyUp { code: 56 },
        ]
        .into_iter()
        .enumerate()
        {
            tx.send(ViewerInput::Control(DesktopControl::Input(InputEvent {
                seq: seq as u64,
                event_ts_ms: 10,
                display_id: 0,
                kind,
            })))
            .await
            .unwrap();
        }
        let mut observed = Vec::new();
        let mut heartbeats = 0;
        while observed.len() < 4 || heartbeats < 2 {
            let message: DesktopUp = tokio::time::timeout(
                Duration::from_millis(1200),
                rds_net::read_frame(&mut remote),
            )
            .await
            .unwrap()
            .unwrap();
            match message {
                DesktopUp::Control(DesktopControl::Input(event)) => observed.push(event),
                DesktopUp::Control(DesktopControl::Heartbeat { .. }) => heartbeats += 1,
                other => panic!("unexpected control: {other:?}"),
            }
        }
        assert_eq!(
            observed.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert!(started.elapsed() < Duration::from_millis(1200));
        assert!(
            !dropped.load(Ordering::SeqCst),
            "decoder must stay held during dispatch"
        );
        tx.send(ViewerInput::Close).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        );
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_decode_cannot_disable_progress_watchdog() {
        let (wire, mut remote) = tokio::io::duplex(256);
        let (_tx, mut input) = mpsc::channel(1);
        let stop = CancellationToken::new();
        let task = tokio::spawn(async move {
            let (_progress, last_frame) = watch::channel(tokio::time::Instant::now());
            run(
                pump(
                    &mut input,
                    &Wire(Mutex::new(wire)),
                    &last_frame,
                    Instant::now(),
                    |_| {},
                ),
                std::future::pending(),
                &stop,
            )
            .await
        });
        let started = tokio::time::Instant::now();
        for _ in 0..15 {
            let _: DesktopUp = rds_net::read_frame(&mut remote).await.unwrap();
        }
        let error = task.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("video stopped making progress"));
        assert_eq!(started.elapsed(), Duration::from_secs(15));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_control_write_is_bounded_and_cancel_releases_both_legs() {
        for cancel in [false, true] {
            let (wire, _remote) = tokio::io::duplex(1);
            let (tx, mut input) = mpsc::channel(1);
            tx.send(ViewerInput::Control(DesktopControl::RequestIdr))
                .await
                .unwrap();
            let stop = CancellationToken::new();
            let canceled = stop.clone();
            let dropped = Arc::new(AtomicBool::new(false));
            let held = dropped.clone();
            let (decoding, started_decode) = oneshot::channel();
            let task = tokio::spawn(async move {
                let (_progress, last_frame) = watch::channel(tokio::time::Instant::now());
                let media = async {
                    let _drop = Dropped(held);
                    decoding.send(()).unwrap();
                    std::future::pending().await
                };
                run(
                    pump(
                        &mut input,
                        &Wire(Mutex::new(wire)),
                        &last_frame,
                        Instant::now(),
                        |_| {},
                    ),
                    media,
                    &stop,
                )
                .await
            });
            started_decode.await.unwrap();
            let started = tokio::time::Instant::now();
            if cancel {
                canceled.cancel();
            }
            let result = task.await.unwrap();
            if cancel {
                assert!(result.unwrap());
                assert_eq!(started.elapsed(), Duration::ZERO);
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("control write stalled")
                );
                assert_eq!(started.elapsed(), Duration::from_secs(2));
            }
            assert!(dropped.load(Ordering::SeqCst));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fresh_decoded_progress_renews_watchdog_without_restarting_control_leg() {
        let (wire, mut remote) = tokio::io::duplex(256);
        let (tx, mut input) = mpsc::channel(1);
        let (progress, last_frame) = watch::channel(tokio::time::Instant::now());
        let stop = CancellationToken::new();
        let task = tokio::spawn(async move {
            run(
                pump(
                    &mut input,
                    &Wire(Mutex::new(wire)),
                    &last_frame,
                    Instant::now(),
                    |_| {},
                ),
                std::future::pending(),
                &stop,
            )
            .await
        });
        let started = tokio::time::Instant::now();
        for second in 0..31 {
            let _: DesktopUp = rds_net::read_frame(&mut remote).await.unwrap();
            if second == 10 || second == 20 {
                progress.send_replace(tokio::time::Instant::now());
            }
        }
        assert_eq!(started.elapsed(), Duration::from_secs(30));
        assert!(!task.is_finished());
        tx.send(ViewerInput::Close).await.unwrap();
        assert!(task.await.unwrap().unwrap());
    }
}
