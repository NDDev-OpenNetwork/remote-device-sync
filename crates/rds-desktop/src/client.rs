//! Viewing side of a desktop session.
//!
//! `DesktopSession` owns the control stream (input + encoder steering) and
//! a receiver of decoded frames. Frame streams that arrive stale are reset
//! immediately, so a slow link degrades to lower effective fps rather than
//! queueing latency.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rds_core::{
    Codec, DesktopCaps, DesktopControl, DesktopHello, FrameHeader, HelloAck, InputEvent,
    StreamHello, read_frame, write_frame,
};
use rds_net::Connection;
use tokio::sync::mpsc;

use crate::{DesktopError, RawFrame};

/// A live desktop session on the client side.
pub struct DesktopSession {
    /// Decoded frames in arrival order; stale ones never arrive here.
    pub frames: mpsc::Receiver<RawFrame>,
    /// Send input or encoder control to the serving side.
    ctrl_tx: mpsc::Sender<DesktopControl>,
    /// Highest frame sequence fully received so far.
    last_seq: Arc<AtomicU64>,
    caps: DesktopCaps,
    /// Frame-receiver task; it holds a `Connection` clone, so without
    /// aborting it on drop the connection — and the remote session —
    /// would outlive the session forever.
    frame_task: tokio::task::JoinHandle<()>,
}

impl Drop for DesktopSession {
    fn drop(&mut self) {
        self.frame_task.abort();
    }
}

impl DesktopSession {
    /// Open a session on an established connection.
    pub async fn connect(
        conn: &Connection,
        display: u32,
        max_fps: u32,
        codec: Codec,
    ) -> Result<Self, DesktopError> {
        let (mut send, mut recv) = conn.open_bi().await?;
        write_frame(
            &mut send,
            &StreamHello::Desktop(DesktopHello {
                display,
                max_fps,
                codec,
            }),
        )
        .await?;
        let caps = match read_frame::<_, HelloAck>(&mut recv).await? {
            HelloAck::Desktop(caps) => caps,
            HelloAck::Ok => DesktopCaps {
                displays: vec![],
                codecs: vec![codec],
            },
            HelloAck::Error { message } => return Err(DesktopError::Capture(message)),
            HelloAck::Info(_) => return Err(DesktopError::Capture("unexpected ack".into())),
        };

        let (frame_tx, frames) = mpsc::channel(4);
        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<DesktopControl>(64);
        let last_seq = Arc::new(AtomicU64::new(0));

        // Control writer task.
        tokio::spawn(async move {
            while let Some(msg) = ctrl_rx.recv().await {
                if write_frame(&mut send, &msg).await.is_err() {
                    break;
                }
            }
        });

        // Frame receiver task: accept uni streams, drop stale, decode newest.
        let conn = conn.clone();
        let seq_marker = last_seq.clone();
        let frame_task = tokio::spawn(async move {
            while let Ok(mut stream) = conn.accept_uni().await {
                let frame_tx = frame_tx.clone();
                let seq_marker = seq_marker.clone();
                tokio::spawn(async move {
                    let header: FrameHeader = match read_frame(&mut stream).await {
                        Ok(h) => h,
                        Err(_) => return,
                    };
                    let newest = seq_marker.load(Ordering::Relaxed);
                    if header.seq < newest {
                        // A fresher frame already landed; drop the tail.
                        return;
                    }
                    let body = match stream.read_to_end(64 * 1024 * 1024).await {
                        Ok(b) => b,
                        Err(_) => return,
                    };
                    seq_marker.store(header.seq, Ordering::Relaxed);
                    deliver(header, body, frame_tx).await;
                });
            }
        });

        Ok(Self {
            frames,
            ctrl_tx,
            last_seq,
            caps,
            frame_task,
        })
    }

    pub fn caps(&self) -> &DesktopCaps {
        &self.caps
    }

    /// Highest frame sequence fully received so far.
    pub fn latest_seq(&self) -> u64 {
        self.last_seq.load(Ordering::Relaxed)
    }

    /// Queue one input event for the serving side.
    pub async fn send_input(&self, event: InputEvent) -> Result<(), DesktopError> {
        self.ctrl_tx
            .send(DesktopControl::Input(event))
            .await
            .map_err(|_| DesktopError::Input("control channel closed".into()))
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

async fn deliver(header: FrameHeader, body: Vec<u8>, tx: mpsc::Sender<RawFrame>) {
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
                let _ = tx.try_send(raw);
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
