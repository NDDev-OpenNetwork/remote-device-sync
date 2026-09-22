//! Viewing side of a desktop session.
//!
//! `DesktopSession` owns the control stream (input + encoder steering +
//! heartbeats) and a receiver of decoded frames. Frame streams that arrive
//! stale are reset immediately, so a slow link degrades to lower effective
//! fps rather than queueing latency; when the decode queue is full the
//! *oldest* queued frame is evicted — newest wins.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rds_core::{
    Codec, DesktopCaps, DesktopControl, DesktopEvent, DesktopHello, FrameHeader, HelloAck,
    InputEvent, InputKind, StreamHello, read_frame, write_frame,
};
use rds_net::Connection;
use tokio::sync::mpsc;

use crate::{DesktopError, RawFrame, SessionClock, mailbox};

/// A live desktop session on the client side.
pub struct DesktopSession {
    /// Decoded frames in arrival order; stale ones never arrive here.
    /// Bounded, newest-wins: a full queue evicts the oldest frame.
    pub frames: mailbox::Receiver<RawFrame>,
    /// Headers of every frame that survived the stale-drop check and
    /// arrived complete — the wire-level tap tests and tooling use to
    /// measure latency without a decoder.
    pub frame_headers: mailbox::Receiver<FrameHeader>,
    /// Server→client control events (input acks, heartbeat echoes).
    pub events: mailbox::Receiver<DesktopEvent>,
    /// Send input or encoder control to the serving side.
    ctrl_tx: mpsc::Sender<DesktopControl>,
    /// Next expected frame sequence — the lowest seq still accepted.
    next_seq: Arc<AtomicU64>,
    /// Next input-event sequence number (ack correlation).
    input_seq: AtomicU64,
    /// Next heartbeat sequence number.
    heartbeat_seq: AtomicU64,
    /// Latest control-plane RTT measured via heartbeat echo (ms).
    control_rtt_ms: Arc<AtomicU64>,
    caps: DesktopCaps,
    display: u32,
    clock: SessionClock,
    /// Frame-receiver task; it holds a `Connection` clone, so without
    /// aborting it on drop the connection — and the remote session —
    /// would outlive the session forever.
    frame_task: tokio::task::JoinHandle<()>,
    /// Server-events reader; same lifetime argument.
    event_task: tokio::task::JoinHandle<()>,
}

impl Drop for DesktopSession {
    fn drop(&mut self) {
        self.frame_task.abort();
        self.event_task.abort();
    }
}

/// Options for [`DesktopSession::connect_opts`].
#[derive(Default)]
pub struct SessionOpts {
    /// Override the session clock. Tests pass the same `SessionClock`
    /// to both peers so `FrameHeader` timestamps compare directly to
    /// local receive times; production leaves each side on its own
    /// clock (cross-machine latency then needs an RTT estimate).
    pub clock: Option<SessionClock>,
}

impl DesktopSession {
    /// Open a session on an established connection.
    pub async fn connect(
        conn: &Connection,
        display: u32,
        max_fps: u32,
        codec: Codec,
    ) -> Result<Self, DesktopError> {
        Self::connect_opts(
            conn,
            DesktopHello {
                display,
                max_fps,
                codec,
                input_acks: false,
            },
            SessionOpts::default(),
        )
        .await
    }

    /// Open a session with the full hello (input acks) and options.
    pub async fn connect_opts(
        conn: &Connection,
        hello: DesktopHello,
        opts: SessionOpts,
    ) -> Result<Self, DesktopError> {
        let display = hello.display;
        let clock = opts.clock.unwrap_or_default();
        let (mut send, mut recv) = conn.open_bi().await?;
        write_frame(&mut send, &StreamHello::Desktop(hello)).await?;
        let caps = match read_frame::<_, HelloAck>(&mut recv).await? {
            HelloAck::Desktop(caps) => caps,
            HelloAck::Ok => DesktopCaps {
                displays: vec![],
                codecs: vec![],
            },
            HelloAck::Error { message } => return Err(DesktopError::Capture(message)),
            HelloAck::Info(_) => return Err(DesktopError::Capture("unexpected ack".into())),
        };

        let (frame_tx, frames) = mailbox::channel(4);
        let (header_tx, frame_headers) = mailbox::channel(64);
        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<DesktopControl>(64);
        let (events_tx, events) = mailbox::channel::<DesktopEvent>(128);
        let next_seq = Arc::new(AtomicU64::new(0));
        let control_rtt_ms = Arc::new(AtomicU64::new(0));

        // Control writer task: single writer on `send`.
        tokio::spawn(async move {
            while let Some(msg) = ctrl_rx.recv().await {
                if write_frame(&mut send, &msg).await.is_err() {
                    break;
                }
            }
        });

        // Server-events reader: input acks + heartbeat echoes land on the
        // same bidi stream after the HelloAck.
        let rtt_marker = control_rtt_ms.clone();
        let event_clock = clock.clone();
        let event_task = tokio::spawn(async move {
            loop {
                match read_frame::<_, DesktopEvent>(&mut recv).await {
                    Ok(ev @ DesktopEvent::Heartbeat { ts_ms, .. }) => {
                        let now = event_clock.now_ms();
                        rtt_marker.store(now.saturating_sub(ts_ms), Ordering::Relaxed);
                        events_tx.send(ev);
                    }
                    Ok(ev) => {
                        events_tx.send(ev);
                    }
                    Err(_) => break,
                }
            }
        });

        // Frame receiver task: accept uni streams, drop stale, decode
        // newest. A delivered-seq gap means a delta chain broke — the
        // session auto-requests an IDR so decode can resync.
        let conn = conn.clone();
        // `next_seq` is the lowest seq still acceptable — the next
        // expected frame. Init 0 accepts the stream's first frame
        // (seq 0 is fresh, not stale) and lets the gap check catch a
        // first arrival above it. `latest_seq()` derives from it.
        let seq_marker = next_seq.clone();
        let gap_ctrl = ctrl_tx.clone();
        // Serializes claim+delivery so the order frames reach the
        // consumer is strictly the order of their seq numbers.
        let deliver_lock = Arc::new(tokio::sync::Mutex::new(()));
        let frame_task = tokio::spawn(async move {
            while let Ok(mut stream) = conn.accept_uni().await {
                let frame_tx = frame_tx.clone();
                let header_tx = header_tx.clone();
                let seq_marker = seq_marker.clone();
                let gap_ctrl = gap_ctrl.clone();
                let deliver_lock = deliver_lock.clone();
                tokio::spawn(async move {
                    let header: FrameHeader = match read_frame(&mut stream).await {
                        Ok(h) => h,
                        Err(_) => return,
                    };
                    if header.seq < seq_marker.load(Ordering::Relaxed) {
                        return;
                    }
                    let body = match stream.read_to_end(64 * 1024 * 1024).await {
                        Ok(b) => b,
                        Err(_) => return,
                    };
                    // Delivered order must match seq order: claim the
                    // watermark and publish under one lock so a slower
                    // body can't deliver after a newer frame landed.
                    let _guard = deliver_lock.lock().await;
                    let expected = seq_marker.load(Ordering::Relaxed);
                    if header.seq < expected {
                        return;
                    }
                    if header.seq > expected {
                        // Delta chain gap: request resync.
                        let _ = gap_ctrl.try_send(DesktopControl::RequestIdr);
                    }
                    seq_marker.store(header.seq + 1, Ordering::Relaxed);
                    // Wire-level tap: every complete, non-stale frame.
                    header_tx.send(header.clone());
                    deliver(header, body, frame_tx.clone()).await;
                });
            }
        });

        Ok(Self {
            frames,
            frame_headers,
            events,
            ctrl_tx,
            next_seq,
            input_seq: AtomicU64::new(0),
            heartbeat_seq: AtomicU64::new(0),
            control_rtt_ms,
            caps,
            display,
            clock,
            frame_task,
            event_task,
        })
    }

    pub fn caps(&self) -> &DesktopCaps {
        &self.caps
    }

    /// Highest frame sequence fully received so far.
    pub fn latest_seq(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed).saturating_sub(1)
    }

    /// Latest control-plane round trip measured by heartbeat echo.
    /// `None` until the first heartbeat returns.
    pub fn control_rtt(&self) -> Option<std::time::Duration> {
        match self.control_rtt_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(std::time::Duration::from_millis(ms)),
        }
    }

    /// Queue one input event for the serving side. Sequence, timestamp
    /// and target display are filled in from session state.
    pub async fn send_input(&self, kind: InputKind) -> Result<u64, DesktopError> {
        let seq = self.input_seq.fetch_add(1, Ordering::Relaxed);
        let event = InputEvent {
            seq,
            event_ts_ms: self.clock.now_ms(),
            display_id: self.display,
            kind,
        };
        self.ctrl_tx
            .send(DesktopControl::Input(event))
            .await
            .map_err(|_| DesktopError::Input("control channel closed".into()))?;
        Ok(seq)
    }

    /// Liveness probe: the server echoes it; `control_rtt()` reflects
    /// the round trip once the echo lands.
    pub async fn heartbeat(&self) -> Result<u64, DesktopError> {
        let seq = self.heartbeat_seq.fetch_add(1, Ordering::Relaxed);
        self.ctrl_tx
            .send(DesktopControl::Heartbeat {
                seq,
                ts_ms: self.clock.now_ms(),
            })
            .await
            .map_err(|_| DesktopError::Input("control channel closed".into()))?;
        Ok(seq)
    }

    /// Ask the encoder for a fresh keyframe.
    pub async fn request_idr(&self) -> Result<(), DesktopError> {
        self.ctrl_tx
            .send(DesktopControl::RequestIdr)
            .await
            .map_err(|_| DesktopError::Input("control channel closed".into()))
    }

    /// Request an encoder bitrate in bits per second.
    pub async fn set_bitrate(&self, bps: u32) -> Result<(), DesktopError> {
        self.ctrl_tx
            .send(DesktopControl::SetBitrate(bps))
            .await
            .map_err(|_| DesktopError::Input("control channel closed".into()))
    }
}

async fn deliver(header: FrameHeader, body: Vec<u8>, tx: mailbox::Sender<RawFrame>) {
    #[cfg(feature = "x11")]
    {
        use crate::Decoder;
        use std::sync::Mutex;
        static DECODER: Mutex<Option<crate::H264Decoder>> = Mutex::new(None);
        let encoded = crate::EncodedFrame {
            codec: header.codec,
            data: bytes::Bytes::from(body),
            keyframe: header.keyframe,
        };
        let mut guard = match DECODER.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.is_none() {
            *guard = match crate::H264Decoder::new() {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!("decoder init failed: {e}");
                    return;
                }
            };
        }
        match guard.as_mut().unwrap().decode(&encoded) {
            Ok(Some(raw)) => {
                tracing::debug!(
                    "frame seq={} {}x{} decoded",
                    header.seq,
                    raw.width,
                    raw.height
                );
                // Newest-frame-wins: a full queue evicts the oldest
                // frame rather than dropping the fresh one.
                tx.send(raw);
            }
            Ok(None) => tracing::debug!("frame seq={} buffered", header.seq),
            Err(e) => tracing::debug!("decode seq={} failed: {e}", header.seq),
        }
    }
    #[cfg(not(feature = "x11"))]
    {
        let _ = (header, body, tx);
    }
}

/// Headless desktop run: connects, prints capabilities, streams decode
/// stats until interrupted. A GUI front-end consumes `session.frames`.
pub async fn run_desktop_client(
    conn: Connection,
    display: u32,
    max_fps: u32,
) -> Result<(), DesktopError> {
    let mut session = DesktopSession::connect(&conn, display, max_fps, Codec::H264).await?;
    println!("desktop caps: {:?}", session.caps());
    let mut count = 0u64;
    let start = std::time::Instant::now();
    while let Some(frame) = session.frames.recv().await {
        count += 1;
        if count.is_multiple_of(30) {
            let secs = start.elapsed().as_secs_f64();
            println!(
                "decoded {count} frames, {:.1} fps, last {}x{}",
                count as f64 / secs,
                frame.width,
                frame.height
            );
        }
    }
    Ok(())
}
