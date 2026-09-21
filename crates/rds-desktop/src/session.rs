//! Serving side of a desktop session.
//!
//! Frame delivery follows the MoQ pattern: every encoded frame goes out on
//! its own uni-directional stream carrying a `FrameHeader`, newer frames
//! get higher stream priority, and the peer resets streams overtaken by
//! fresher ones. Input events arrive on the bi-directional control stream.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use rds_core::{DesktopControl, DesktopHello, FrameHeader, read_frame, write_frame};
use tokio::sync::mpsc;
use tracing::debug;

use crate::DesktopError;

/// Highest input/control priority; video frames rank below.
const CONTROL_PRIORITY: i32 = i32::MAX;

struct Produced {
    header: FrameHeader,
    payload: bytes::Bytes,
}

/// Serve one desktop session on an already-accepted stream pair.
///
/// `conn` is needed to open per-frame uni streams back to the viewer.
pub async fn serve_desktop(
    conn: Connection,
    send: SendStream,
    mut recv: RecvStream,
    hello: DesktopHello,
) -> Result<(), DesktopError> {
    send.set_priority(CONTROL_PRIORITY)?;
    let max_fps = hello.max_fps.clamp(1, 60);
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(max_fps));
    let display = hello.display;
    let bitrate = Arc::new(AtomicU64::new(4_000_000));
    let idr = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // Capture+encode runs on a blocking thread; frames flow to the writer.
    let (tx, mut rx) = mpsc::channel::<Produced>(2);
    let enc_bitrate = bitrate.clone();
    let enc_idr = idr.clone();
    let capture_task = tokio::task::spawn_blocking(move || {
        produce_loop(display, frame_interval, enc_bitrate, enc_idr, tx)
    });

    // Writer task: one uni stream per frame. When several frames are
    // queued, collapse them to the newest — a stale frame is latency,
    // not information.
    let writer_conn = conn.clone();
    let mut writer = tokio::spawn(async move {
        while let Some(mut produced) = rx.recv().await {
            while let Ok(newer) = rx.try_recv() {
                produced = newer;
            }
            let conn = writer_conn.clone();
            tokio::spawn(async move {
                if let Err(e) = write_frame_stream(conn, produced).await {
                    debug!("frame stream dropped: {e}");
                }
            });
        }
    });

    // Control loop: input + encoder steering, until the peer goes away.
    let control = async {
        loop {
            match read_frame::<_, DesktopControl>(&mut recv).await {
                Ok(DesktopControl::Input(ev)) => {
                    #[cfg(all(target_os = "linux", feature = "x11"))]
                    if let Err(e) = crate::input::x11::inject(&ev) {
                        tracing::warn!("input injection failed: {e}");
                    }
                    #[cfg(not(all(target_os = "linux", feature = "x11")))]
                    let _ = ev;
                }
                Ok(DesktopControl::RequestIdr) => idr.store(true, Ordering::Relaxed),
                Ok(DesktopControl::SetBitrate(bps)) => {
                    bitrate.store(u64::from(bps.max(50_000)), Ordering::Relaxed)
                }
                Err(_) => break, // peer closed the control stream
            }
        }
    };

    let result = tokio::select! {
        _ = control => Ok(()),
        res = &mut writer => match res {
            Ok(()) => Ok(()),
            Err(e) => Err(DesktopError::Io(std::io::Error::other(e.to_string()))),
        },
    };
    capture_task.abort();
    result
}

fn produce_loop(
    display: u32,
    interval: Duration,
    bitrate: Arc<AtomicU64>,
    idr: Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<Produced>,
) {
    #[cfg(not(all(target_os = "linux", feature = "x11")))]
    {
        let _ = (display, interval, bitrate, idr, tx);
    }
    #[cfg(all(target_os = "linux", feature = "x11"))]
    produce_loop_x11(display, interval, bitrate, idr, tx)
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn produce_loop_x11(
    display: u32,
    interval: Duration,
    bitrate: Arc<AtomicU64>,
    idr: Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<Produced>,
) {
    use std::time::Instant;

    use crate::capture::x11::X11Capturer;
    use crate::codec::openh264::H264Encoder;
    use crate::{Capturer, Encoder};

    let mut capturer = match X11Capturer::new(display) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("capture init failed: {e}");
            return;
        }
    };
    let fps = 1.0 / interval.as_secs_f32();
    let mut encoder = match H264Encoder::new(bitrate.load(Ordering::Relaxed), fps) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("encoder init failed: {e}");
            return;
        }
    };
    let start = Instant::now();
    let mut seq = 0u64;
    loop {
        let due = start + interval.mul_f64(seq as f64);
        if let Some(sleep) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(sleep);
        }
        let raw = match capturer.capture() {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("capture failed: {e}");
                return;
            }
        };
        if idr.swap(false, Ordering::Relaxed) {
            encoder.request_idr();
        }
        encoder.set_bitrate(bitrate.load(Ordering::Relaxed).min(u64::from(u32::MAX)) as u32);
        let (width, height) = (raw.width, raw.height);
        match encoder.encode(&raw) {
            Ok(frame) => {
                let produced = Produced {
                    header: FrameHeader {
                        seq,
                        pts_ms: start.elapsed().as_millis() as u64,
                        keyframe: frame.keyframe,
                        width,
                        height,
                        codec: frame.codec,
                    },
                    payload: frame.data,
                };
                // Bounded channel; when full the writer is behind and
                // this frame is dropped — its successor lands fresher.
                let _ = tx.try_send(produced);
                seq += 1;
            }
            Err(e) => tracing::warn!("encode failed: {e}"),
        }
    }
}

async fn write_frame_stream(conn: Connection, produced: Produced) -> Result<(), DesktopError> {
    let mut stream = conn.open_uni().await?;
    stream.set_priority(stream_priority(produced.header.seq))?;
    write_frame(&mut stream, &produced.header).await?;
    stream.write_all(&produced.payload).await?;
    stream.finish()?;
    Ok(())
}

/// Newer frames outrank older ones; wrap to stay positive.
fn stream_priority(seq: u64) -> i32 {
    (seq % (i32::MAX as u64)) as i32
}
