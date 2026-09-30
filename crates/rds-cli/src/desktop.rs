//! Native viewer orchestration. The OS event loop owns the main thread; one
//! cancelable async worker owns the remote desktop and bounded decode work.
use clap::Args;
use rds_client::local::Client;
use rds_core::local::SessionId;
#[cfg(feature = "desktop")]
use rds_core::local::{Command, Reply};
#[cfg(feature = "desktop")]
use rds_core::{Codec, DesktopHello};
#[cfg(feature = "desktop")]
use std::time::Duration;
use std::{
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
};

#[derive(Args, Clone)]
pub struct Options {
    #[arg(long, default_value = "0")]
    pub display: u32,
    #[arg(long, default_value = "60")]
    pub max_fps: NonZeroU32,
    /// Decode/statistics without opening a native window.
    #[arg(long, conflicts_with = "report")]
    pub headless: bool,
    /// End after this many seconds; useful for bounded qualification runs.
    #[arg(long)]
    pub duration: Option<NonZeroU64>,
    /// Write native presentation diagnostics to a new JSON file.
    #[arg(long)]
    pub report: Option<PathBuf>,
}

/// Shared bounded credential loading for CLI and native application clients.
pub async fn read_grant(
    path: Option<PathBuf>,
) -> anyhow::Result<Option<Box<rds_core::grant::Grant>>> {
    match path {
        Some(path) => Ok(Some(Box::new(
            tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                use rustix::fs::{Mode, OFlags};
                use std::io::Read;
                let file = std::fs::File::from(rustix::fs::open(
                    &path,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )?);
                anyhow::ensure!(file.metadata()?.is_file(), "grant must be a regular file");
                let mut bytes = Vec::new();
                file.take(65537).read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 65536, "grant file exceeds 64 KiB");
                Ok(serde_json::from_slice(&bytes)?)
            })
            .await??,
        ))),
        None => Ok(None),
    }
}

#[cfg(feature = "desktop")]
mod native {
    use super::*;
    use rds_client::local::ManagedMessage;
    use rds_desktop::{
        client::{DesktopSession, RelayDecoder, RelayOutcome, SessionOpts},
        render::{InputReceiver, Viewer, ViewerHandle, ViewerInput},
    };
    use std::time::Instant;
    use tokio_util::sync::CancellationToken;

    pub(super) enum Source {
        Managed {
            client: Client,
            session: Option<SessionId>,
            peer: String,
            grant: Option<Box<rds_core::grant::Grant>>,
        },
        Direct {
            endpoint: rds_net::Endpoint,
            target: rds_net::EndpointAddr,
            grant: Option<rds_core::grant::Grant>,
        },
    }
    fn hello(options: &Options) -> DesktopHello {
        DesktopHello {
            display: options.display,
            max_fps: options.max_fps.get().min(240),
            codec: Codec::H264,
            input_acks: true,
        }
    }
    fn extent(
        handle: &ViewerHandle,
        caps: &rds_core::DesktopCaps,
        display: u32,
    ) -> anyhow::Result<()> {
        let screen = caps
            .displays
            .iter()
            .find(|d| d.index == display)
            .ok_or_else(|| anyhow::anyhow!("remote display {display} unavailable"))?;
        handle.display_extent(screen.width, screen.height);
        Ok(())
    }
    pub(super) async fn run(source: Source, options: Options) -> anyhow::Result<()> {
        if options.headless {
            let duration = options.duration;
            return tokio::select! {
                result = headless(source, options) => result,
                _ = async { match duration { Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds.get())).await, None => std::future::pending().await } } => Ok(()),
            };
        }
        let (viewer, handle, mut input) = Viewer::new(options.display)?;
        let stop = CancellationToken::new();
        let mut workers = tokio::task::JoinSet::new();
        let worker_stop = stop.clone();
        let worker_view = handle.clone();
        let worker_options = options.clone();
        workers.spawn(async move {
            let result = network(
                source,
                &worker_options,
                &worker_view,
                &mut input,
                worker_stop,
            )
            .await;
            if let Err(error) = &result {
                worker_view.status(format!("Disconnected: {error}"));
            }
            result
        });
        if let Some(seconds) = options.duration {
            let view = handle.clone();
            let canceled = stop.clone();
            workers.spawn(async move {
                tokio::select! {
                    _ = canceled.cancelled() => {},
                    _ = tokio::time::sleep(Duration::from_secs(seconds.get())) => view.close(),
                }
                Ok(())
            });
        }
        let result = tokio::task::block_in_place(|| viewer.run());
        stop.cancel();
        if tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(joined) = workers.join_next().await {
                if let Ok(Err(error)) = joined {
                    tracing::warn!(%error,"desktop worker ended");
                }
            }
        })
        .await
        .is_err()
        {
            workers.shutdown().await;
        }
        let report = handle.report();
        if let Some(path) = options.report {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            serde_json::to_writer_pretty(&mut file, &report)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        result?;
        anyhow::ensure!(
            report.frames_submitted > 0,
            "desktop ended before a frame was presented"
        );
        Ok(())
    }

    async fn network(
        mut source: Source,
        options: &Options,
        view: &ViewerHandle,
        input: &mut InputReceiver,
        stop: CancellationToken,
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        let mut failures = 0u32;
        loop {
            let received = view.report().frames_received;
            let result = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                result = async {
                    match &mut source {
                        Source::Managed { client,session,peer,grant } => {
                            if session.is_none() || client.selected(*session).await.is_err() {
                                let Reply::Connected(id) = client.request(Command::Connect { target:peer.clone(),grant:grant.clone() }).await? else { anyhow::bail!("unexpected local connection response"); };
                                // After the first authenticated connect retain the exact peer
                                // identity, even when the initial target was a registry name.
                                let snapshot = client.snapshot().await?;
                                *peer = snapshot.sessions.into_iter().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("connected session disappeared"))?.peer;
                                *session = Some(id);
                            }
                            managed_session(client,session.ok_or_else(|| anyhow::anyhow!("no managed session"))?,options,view,input,&stop,started).await
                        },
                        Source::Direct { endpoint,target,grant } => direct_session(endpoint,(target.clone(),grant.as_ref()),options,view,input,&stop,started).await,
                    }
                } => result,
            };
            if stop.is_cancelled() || matches!(result, Ok(true)) {
                return Ok(());
            }
            if view.report().frames_received > received {
                failures = 0;
            }
            failures = failures.saturating_add(1);
            view.status("Reconnecting");
            tracing::debug!(error = ?result.as_ref().err(),"desktop reconnecting");
            let wait = Duration::from_millis((250u64 << failures.min(5)).min(8000));
            if !retry_pause(wait, &stop, input).await {
                return Ok(());
            }
            // Never replay input captured while disconnected onto a replacement.
            while let Some(message) = input.try_recv() {
                if matches!(message, ViewerInput::Close) {
                    return Ok(());
                }
            }
        }
    }

    trait RetryInput: Send {
        fn close_requested(&mut self) -> impl Future<Output = bool> + Send;
    }
    impl RetryInput for InputReceiver {
        async fn close_requested(&mut self) -> bool {
            matches!(self.recv().await, Some(ViewerInput::Close) | None)
        }
    }

    async fn retry_pause(
        wait: Duration,
        stop: &CancellationToken,
        input: &mut impl RetryInput,
    ) -> bool {
        // Discarded input must neither complete nor restart the retry timer.
        let deadline = tokio::time::sleep(wait);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => return false,
                _ = &mut deadline => return true,
                close = input.close_requested() => if close { return false; },
            }
        }
    }

    async fn managed_session(
        client: &Client,
        session: SessionId,
        options: &Options,
        view: &ViewerHandle,
        input: &mut InputReceiver,
        stop: &CancellationToken,
        started: Instant,
    ) -> anyhow::Result<bool> {
        let mut channel = client.desktop(Some(session), hello(options)).await?;
        extent(view, &channel.caps, options.display)?;
        view.status("Waiting for screen");
        let control = channel.control_handle();
        let mut decoder = RelayDecoder::new();
        let mut last_frame = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => break Ok(true),
                message = input.recv() => match message {
                    Some(ViewerInput::Control(message)) => control.control(message).await?,
                    Some(ViewerInput::Close)|None => break Ok(true),
                },
                _ = tick.tick() => {
                    anyhow::ensure!(last_frame.elapsed() < Duration::from_secs(15),"remote video stopped making progress");
                    control.control(rds_core::DesktopControl::Heartbeat { seq: 0,ts_ms: started.elapsed().as_millis() as u64 }).await?;
                },
                message = channel.recv() => match message? {
                    None => break Ok(false),
                    Some(ManagedMessage::Event(rds_core::DesktopEvent::Heartbeat { ts_ms,.. })) => view.control_rtt((started.elapsed().as_millis() as u64).saturating_sub(ts_ms)),
                    Some(ManagedMessage::Event(rds_core::DesktopEvent::InputAck { .. })) => view.input_ack(),
                    Some(ManagedMessage::Frame(frame)) => {
                        let received = Instant::now();
                        view.media_timing(&frame.header);
                        let (next,outcome) = decoder.push_bounded(frame.header,frame.payload).await?;
                        decoder = next;
                        match outcome {
                            RelayOutcome::Frame(raw) => { last_frame = Instant::now(); view.frame(raw,received); },
                            RelayOutcome::NeedIdr => control.request_idr().await?,
                            RelayOutcome::Pending => {},
                        }
                    }
                }
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(1), channel.finish()).await;
        result
    }

    async fn direct_session(
        endpoint: &rds_net::Endpoint,
        target: (rds_net::EndpointAddr, Option<&rds_core::grant::Grant>),
        options: &Options,
        view: &ViewerHandle,
        input: &mut InputReceiver,
        stop: &CancellationToken,
        started: Instant,
    ) -> anyhow::Result<bool> {
        let (target, grant) = target;
        let conn = match grant {
            Some(g) => rds_client::connect_authorized(endpoint, target, g).await?,
            None => rds_client::connect(endpoint, target).await?,
        };
        struct Close(rds_net::Connection);
        impl Drop for Close {
            fn drop(&mut self) {
                self.0.close(0u32.into(), b"viewer session ended");
            }
        }
        let _close = Close(conn.clone());
        let mut session = DesktopSession::connect_opts(
            &conn,
            hello(options),
            SessionOpts {
                session: Some(rand_id()),
                ..Default::default()
            },
        )
        .await?;
        extent(view, session.caps(), options.display)?;
        view.status("Waiting for screen");
        let ctrl = session.control_sender();
        let mut last_frame = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => break Ok(true),
                message = input.recv() => match message {
                    Some(ViewerInput::Control(message)) => ctrl.send(message).await?,
                    Some(ViewerInput::Close)|None => break Ok(true),
                },
                _ = tick.tick() => {
                    anyhow::ensure!(last_frame.elapsed() < Duration::from_secs(15),"remote video stopped making progress");
                    ctrl.send(rds_core::DesktopControl::Heartbeat { seq: 0,ts_ms: started.elapsed().as_millis() as u64 }).await?;
                },
                event = session.events.recv() => match event {
                    Some(rds_core::DesktopEvent::Heartbeat { ts_ms,.. }) => view.control_rtt((started.elapsed().as_millis() as u64).saturating_sub(ts_ms)),
                    Some(rds_core::DesktopEvent::InputAck { .. }) => view.input_ack(),
                    None => break Ok(false),
                },
                frame = session.frames.recv() => match frame {
                    Some(raw) => { last_frame = Instant::now(); view.frame(raw,Instant::now()); },
                    None => break Ok(false),
                },
            }
        };
        drop(session);
        conn.close(0u32.into(), b"viewer session ended");
        result
    }
    fn rand_id() -> [u8; 16] {
        rand::random()
    }

    async fn headless(source: Source, options: Options) -> anyhow::Result<()> {
        match source {
            Source::Direct {
                endpoint,
                target,
                grant,
            } => {
                let conn = match grant {
                    Some(g) => rds_client::connect_authorized(&endpoint, target, &g).await?,
                    None => rds_client::connect(&endpoint, target).await?,
                };
                rds_desktop::client::run_desktop_client(
                    conn,
                    options.display,
                    options.max_fps.get(),
                )
                .await?;
            }
            Source::Managed {
                client,
                session,
                peer,
                grant,
            } => {
                let session = match session {
                    Some(session) => session,
                    None => match client
                        .request(Command::Connect {
                            target: peer,
                            grant,
                        })
                        .await?
                    {
                        Reply::Connected(session) => session,
                        _ => anyhow::bail!("unexpected local connection response"),
                    },
                };
                let mut channel = client.desktop(Some(session), hello(&options)).await?;
                let mut decoder = RelayDecoder::new();
                let mut count = 0u64;
                loop {
                    match channel.recv().await? {
                        Some(ManagedMessage::Frame(frame)) => {
                            let (next, outcome) =
                                decoder.push_bounded(frame.header, frame.payload).await?;
                            decoder = next;
                            match outcome {
                                RelayOutcome::Frame(raw) => {
                                    count += 1;
                                    if count.is_multiple_of(30) {
                                        println!(
                                            "decoded {count} frames, {}x{}",
                                            raw.width, raw.height
                                        );
                                    }
                                }
                                RelayOutcome::NeedIdr => channel.request_idr().await?,
                                RelayOutcome::Pending => {}
                            }
                        }
                        Some(ManagedMessage::Event(_)) => {}
                        None => break,
                    }
                }
            }
        }
        Ok(())
    }
    #[cfg(test)]
    mod retry_tests {
        use super::*;

        impl RetryInput for tokio::sync::mpsc::Receiver<bool> {
            async fn close_requested(&mut self) -> bool {
                self.recv().await.unwrap_or(true)
            }
        }

        #[tokio::test(start_paused = true)]
        async fn input_does_not_end_or_extend_retry_delay() {
            let stop = CancellationToken::new();
            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
            let wait = Duration::from_secs(8);
            let started = tokio::time::Instant::now();
            let mut retry = Box::pin(retry_pause(wait, &stop, &mut rx));
            tx.send(false).await.unwrap();
            tokio::select! {
                result = &mut retry => panic!("input bypassed retry delay: {result}"),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            }
            for _ in 0..6 {
                tx.send(false).await.unwrap();
                tokio::select! {
                    result = &mut retry => panic!("input bypassed retry delay: {result}"),
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                }
            }
            assert!(retry.await);
            assert_eq!(started.elapsed(), wait, "input must not restart the timer");
            assert!(
                rx.try_recv().is_err(),
                "disconnected input was not discarded"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn close_and_cancel_interrupt_retry_immediately() {
            let stop = CancellationToken::new();
            let started = tokio::time::Instant::now();
            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
            tx.send(true).await.unwrap();
            assert!(!retry_pause(Duration::from_secs(8), &stop, &mut rx).await);
            stop.cancel();
            assert!(!retry_pause(Duration::from_secs(8), &stop, &mut rx).await);
            assert_eq!(started.elapsed(), Duration::ZERO);
        }
    }
}

#[cfg(feature = "desktop")]
pub async fn managed(
    client: &Client,
    session: SessionId,
    grant: Option<Box<rds_core::grant::Grant>>,
    options: Options,
) -> anyhow::Result<()> {
    let snapshot = client.snapshot().await?;
    let peer = snapshot
        .sessions
        .into_iter()
        .find(|s| s.id == session)
        .ok_or_else(|| anyhow::anyhow!("session no longer available"))?
        .peer;
    native::run(
        native::Source::Managed {
            client: client.clone(),
            session: Some(session),
            peer,
            grant,
        },
        options,
    )
    .await
}

/// Open the application before attempting network/agent access, so startup
/// outages remain visible and bounded recovery can proceed in the window.
#[cfg(feature = "desktop")]
pub async fn managed_target(
    client: &Client,
    target: String,
    grant: Option<Box<rds_core::grant::Grant>>,
    options: Options,
) -> anyhow::Result<()> {
    native::run(
        native::Source::Managed {
            client: client.clone(),
            session: None,
            peer: target,
            grant,
        },
        options,
    )
    .await
}
#[cfg(feature = "desktop")]
pub async fn direct(
    endpoint: &rds_net::Endpoint,
    target: rds_net::EndpointAddr,
    grant: Option<rds_core::grant::Grant>,
    options: Options,
) -> anyhow::Result<()> {
    native::run(
        native::Source::Direct {
            endpoint: endpoint.clone(),
            target,
            grant,
        },
        options,
    )
    .await
}
#[cfg(not(feature = "desktop"))]
pub async fn managed(
    _client: &Client,
    _session: SessionId,
    _grant: Option<Box<rds_core::grant::Grant>>,
    _options: Options,
) -> anyhow::Result<()> {
    anyhow::bail!("rds built without desktop support; enable the desktop feature")
}
#[cfg(not(feature = "desktop"))]
pub async fn direct(
    _endpoint: &rds_net::Endpoint,
    _target: rds_net::EndpointAddr,
    _grant: Option<rds_core::grant::Grant>,
    _options: Options,
) -> anyhow::Result<()> {
    anyhow::bail!("rds built without desktop support; enable the desktop feature")
}
