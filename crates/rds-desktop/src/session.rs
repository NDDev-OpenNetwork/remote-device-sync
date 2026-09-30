//! Serving side of a desktop session.
//!
//! Frame delivery follows the MoQ pattern: every encoded frame goes out on
//! its own uni-directional stream carrying a `FrameHeader`. Deltas retain
//! their predecessor; only an independent keyframe can replace a delta
//! still in flight. A sequence gap requires a keyframe before delivery
//! resumes. Input events, encoder steering and heartbeats arrive
//! on the bi-directional control stream, which outranks every frame
//! stream.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rds_core::{DesktopControl, DesktopEvent, DesktopHello, FrameHeader};
use rds_net::{Connection, PathStats, RecvStream, SendStream};
use rds_net::{read_frame, write_frame};
use std::borrow::Borrow;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::DesktopError;

use rds_net::wire::{CONTROL_STREAM_PRIORITY, MEDIA_STREAM_PRIORITY};

/// Pacing sample interval for the bitrate controller.
const PACING_INTERVAL: Duration = Duration::from_millis(250);
/// Loss ratio that drives the bitrate down (2%).
const LOSS_STEP_DOWN: f64 = 0.02;
/// New RTT growth over the preceding sample that counts as congestion (1.5×).
const RTT_STEP_UP: f64 = 1.5;
// A queued FIN is not a delivery receipt. Keep fewer unacknowledged media
// streams than the receiver's four readers, leaving capacity for recovery.
const MAX_PENDING_FRAME_ACKS: usize = 3;
const FRAME_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Monotonic clock shared by producer and writer so `FrameHeader`
/// timestamps are comparable within one session.
#[derive(Clone)]
pub struct SessionClock {
    start: Instant,
}

impl Default for SessionClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl SessionClock {
    /// Milliseconds since the session clock started.
    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// One produced frame ready for the writer task.
pub struct Produced {
    pub header: FrameHeader,
    pub payload: Bytes,
}

struct CapturePermit(Arc<AtomicU64>);
impl Drop for CapturePermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}
struct AdmittedFrame {
    produced: Produced,
    permit: CapturePermit,
}
impl Borrow<Produced> for AdmittedFrame {
    fn borrow(&self) -> &Produced {
        &self.produced
    }
}
impl std::ops::Deref for AdmittedFrame {
    type Target = Produced;
    fn deref(&self) -> &Produced {
        &self.produced
    }
}
impl std::ops::DerefMut for AdmittedFrame {
    fn deref_mut(&mut self) -> &mut Produced {
        &mut self.produced
    }
}

/// Live knobs the session applies while a producer runs: the pacing
/// controller writes `bitrate`, the control stream flips `idr` and files
/// `requested` bitrate targets (0 = none pending) for the controller to
/// drain, and the producer reports `deadline_misses` it observed.
pub struct ProducerControls {
    pub bitrate: Arc<AtomicU64>,
    pub idr: Arc<AtomicBool>,
    pub deadline_misses: Arc<AtomicU64>,
    pub requested: Arc<AtomicU64>,
}

impl ProducerControls {
    fn new(initial_bps: u64) -> Self {
        Self {
            bitrate: Arc::new(AtomicU64::new(initial_bps)),
            idr: Arc::new(AtomicBool::new(true)),
            deadline_misses: Arc::new(AtomicU64::new(0)),
            requested: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// A blocking frame source: captures and encodes at its own cadence.
///
/// `produce` runs on a blocking thread; returning `None` ends the video
/// side of the session. Implementations honor `controls.bitrate` /
/// `controls.idr` each call and count a slot missed into
/// `controls.deadline_misses` when a frame lands after its cadence slot.
pub trait FrameProducer: Send + 'static {
    /// Session admission deliberately paused capture. Rebase elapsed cadence
    /// without counting that wait as capture/encoder starvation.
    fn resume_after_backpressure(&mut self) {}
    /// A codec may intentionally emit no frame without changing its reference
    /// chain. Only that case preserves the encoded sequence; capture/encode
    /// errors remain discontinuities requiring resync.
    fn preserves_reference(&self) -> bool {
        false
    }
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced>;
}

/// Everything `serve_desktop` needs beyond the negotiated hello.
#[derive(Default)]
pub struct SessionConfig {
    /// Explicit per-session height overrides the deployment fallback. Zero
    /// keeps native geometry; None uses RDS_DESKTOP_OUTPUT_HEIGHT if supplied.
    pub output_height: Option<u32>,
    /// Hard ceiling for encoder bitrate — the grant's `max_bps`
    /// constraint lands here when the connection is grant-authorized.
    pub bitrate_ceiling: Option<u64>,
    /// Deny input even when a backend is available. Encoder steering and
    /// heartbeats remain usable. The agent derives this from verified grants.
    pub view_only: bool,
    /// Input backend override. `None` lazily probes on the input worker.
    /// Synthetic sessions should supply a synthetic sink, never the host's.
    pub input_sink: Option<Box<dyn crate::InputSink>>,
    /// Frame source override; `None` uses the platform capture backend.
    /// Synthetic sources are how tests pressurize the pipeline.
    pub producer: Option<Box<dyn FrameProducer>>,
    /// Session clock override. Tests share one clock with the client so
    /// header timestamps compare directly to client receive times.
    pub clock: Option<SessionClock>,
    /// Uni-stream route tag frame streams lead with. `DesktopV2`
    /// sessions carry the negotiated `DesktopFrames { id }` route so a
    /// delayed stream from an ended session cannot reach a replacement;
    /// the default `Desktop` tag serves the legacy shared route.
    pub frame_route: Option<rds_core::UniHello>,
}

/// Adaptive bitrate for one session (Sunshine lesson: pace the encoder
/// to *measured* path quality, not a static guess).
///
/// Each `step` consumes one sample window: path counters plus the count
/// of producer deadline misses. Loss, congestion events or RTT growth
/// over the previous observed sample push bitrate down multiplicatively; clean
/// windows probe upward toward the ceiling; deadline misses push down
/// even when the path looks clean (encoder starvation is congestion too).
pub struct BitrateController {
    current: u64,
    floor: u64,
    ceiling: u64,
    previous_rtt_ms: Option<u64>,
    last_sent: u64,
    last_lost: u64,
    last_congestion: u64,
    primed: bool,
    last_path: Option<u64>,
}

impl BitrateController {
    pub fn new(initial: u64, ceiling: u64) -> Self {
        Self {
            current: initial.min(ceiling),
            floor: 100_000.min(ceiling),
            ceiling,
            previous_rtt_ms: None,
            last_sent: 0,
            last_lost: 0,
            last_congestion: 0,
            primed: false,
            last_path: None,
        }
    }

    /// Current target bitrate.
    pub fn current(&self) -> u64 {
        self.current
    }

    /// Steer to a viewer-requested target, clamped to the controller's
    /// own floor and ceiling; adaptation resumes from there.
    pub fn steer(&mut self, bps: u64) {
        self.current = bps.clamp(self.floor, self.ceiling);
    }

    /// One pacing step. `path` is the selected path's counters (delta'd
    /// against the previous call); `deadline_misses` is the count of
    /// produce calls that missed their cadence slot since the last step.
    pub fn step(&mut self, path: Option<PathStats>, deadline_misses: u64) -> u64 {
        let mut next = self.current;
        if let Some(p) = path {
            if self.last_path != Some(p.path_id) {
                self.last_path = Some(p.path_id);
                self.previous_rtt_ms = None;
                self.last_sent = 0;
                self.last_lost = 0;
                self.last_congestion = 0;
                self.primed = false;
            }
            let d_sent = p.sent.saturating_sub(self.last_sent);
            let d_lost = p.lost.saturating_sub(self.last_lost);
            let loss = if d_sent > 0 {
                d_lost as f64 / d_sent as f64
            } else {
                0.0
            };
            let congestion = p.congestion_events > self.last_congestion;
            let rtt_ms = p.rtt.as_millis() as u64;
            let rtt_high = self
                .previous_rtt_ms
                .is_some_and(|b| rtt_ms > (b as f64 * RTT_STEP_UP) as u64);
            if rtt_ms > 0 {
                // A sustained propagation/path delay is not a fresh congestion
                // signal every 250 ms. Reusing the startup RTT permanently
                // drove otherwise clean Full HD streams to the 100 kbps floor.
                self.previous_rtt_ms = Some(rtt_ms);
            }
            self.last_sent = p.sent;
            self.last_lost = p.lost;
            self.last_congestion = p.congestion_events;

            // First sample only establishes the baseline — never react
            // to counters we didn't watch accumulate.
            if self.primed && (loss > LOSS_STEP_DOWN || congestion || rtt_high) {
                next = (next / 10 * 7 + next % 10 * 7 / 10).max(self.floor);
            } else if self.primed && deadline_misses > 0 {
                next = (next / 100 * 85 + next % 100 * 85 / 100).max(self.floor);
            } else if self.primed {
                next = next.saturating_add(next / 10).min(self.ceiling);
            }
            self.primed = true;
        } else if deadline_misses > 0 {
            next = (next / 100 * 85 + next % 100 * 85 / 100).max(self.floor);
        }
        self.current = next;
        next
    }
}

/// Serve one desktop session on an already-accepted stream pair.
///
/// `conn` is needed to open per-frame uni streams back to the viewer.
pub async fn serve_desktop(
    conn: Connection,
    send: SendStream,
    recv: RecvStream,
    hello: DesktopHello,
) -> Result<(), DesktopError> {
    serve_desktop_with(conn, send, recv, hello, SessionConfig::default()).await
}

/// Full session form: explicit config lets callers bound the bitrate
/// (grant constraints) and substitute the frame source (tests, bench).
pub async fn serve_desktop_with(
    conn: Connection,
    send: SendStream,
    mut recv: RecvStream,
    hello: DesktopHello,
    config: SessionConfig,
) -> Result<(), DesktopError> {
    let mut send = SessionSend(send);
    if config
        .output_height
        .is_some_and(|h| h != 0 && !(16..=4320).contains(&h))
    {
        return Err(DesktopError::Capture(
            "video height must be 0 or 16..=4320".into(),
        ));
    }
    send.0.set_priority(CONTROL_STREAM_PRIORITY)?;
    // Dropping the serving future aborts async siblings and queued blocking
    // work. A running capture call may finish, then sees its receiver closed.
    let mut workers = JoinSet::new();
    let mut capture = JoinSet::new();
    let clock = config.clock.clone().unwrap_or_default();
    let max_fps = hello.max_fps.clamp(1, 240);
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(max_fps));
    tracing::info!(display=hello.display,max_fps,output_height=?config.output_height,view_only=config.view_only,"desktop serving started");
    let acks = hello.input_acks;
    if config.bitrate_ceiling.is_some_and(|rate| rate < 100_000) {
        return Err(DesktopError::Encode(
            "granted bitrate is below the supported 100000 bps floor".into(),
        ));
    }
    let ceiling = config
        .bitrate_ceiling
        .unwrap_or(8_000_000)
        .min(u64::from(u32::MAX));
    let controls = ProducerControls::new(4_000_000_u64.min(ceiling));
    // No backend is opened for a view-only session. Blocking platform calls
    // run on one bounded, session-owned worker, outside the async executor.
    let mut input = None;
    let mut input_sink = config.input_sink;

    // Capture+encode runs on a blocking thread; frames flow to the writer.
    let (tx, mut rx) = mpsc::channel::<AdmittedFrame>(2);
    let keyframe_pending = Arc::new(AtomicBool::new(false));
    let capture_admission = Arc::new(AtomicU64::new(0));
    {
        let clock = clock.clone();
        let bitrate = Arc::clone(&controls.bitrate);
        let idr = Arc::clone(&controls.idr);
        let misses = Arc::clone(&controls.deadline_misses);
        let requested = Arc::clone(&controls.requested);
        let mut producer = config.producer;
        let keyframe_pending = keyframe_pending.clone();
        let capture_admission = capture_admission.clone();
        capture.spawn_blocking(move || {
            let producer_controls = ProducerControls {
                bitrate,
                idr,
                deadline_misses: misses,
                // Share the session's target slot — a producer observing
                // `requested` must see what the control arm filed, not a
                // private always-empty copy.
                requested,
            };
            let mut source = match producer.take() {
                Some(p) => p,
                None => platform_producer(hello.display, frame_interval, config.output_height),
            };
            let mut seq = 0u64;
            loop {
                // The channel is the session's lifecycle: when the
                // session ends the writer drops `rx` and this loop exits
                // — abort() cannot interrupt spawn_blocking work.
                if tx.is_closed() {
                    return;
                }
                // Reserve capture cadence through the bounded producer queue
                // before spending CPU or changing the codec's references.
                // Encoding into a full queue used to drop references and
                // force repeated expensive IDRs while QUIC was backlogged.
                let mut paused = false;
                let permit = loop {
                    while tx.capacity() == 0 || keyframe_pending.load(Ordering::Acquire) {
                        paused = true;
                        if tx.is_closed() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    if capture_admission
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                            (count < MAX_PENDING_FRAME_ACKS as u64).then_some(count + 1)
                        })
                        .is_ok()
                    {
                        break CapturePermit(capture_admission.clone());
                    }
                    if tx.is_closed() {
                        return;
                    }
                    paused = true;
                    std::thread::sleep(Duration::from_millis(1));
                };
                if paused {
                    source.resume_after_backpressure();
                }
                match source.produce(seq, &producer_controls, &clock) {
                    Some(p) => {
                        if p.payload.is_empty() && source.preserves_reference() {
                            continue;
                        }
                        if p.header.keyframe && !p.payload.is_empty() {
                            keyframe_pending.store(true, Ordering::Release);
                        }
                        // Losing any encoded reference breaks its successors,
                        // not only losing an IDR. Keep the two-slot bound and
                        // ask the producer for an independent replacement.
                        if tx
                            .try_send(AdmittedFrame {
                                produced: p,
                                permit,
                            })
                            .is_err()
                        {
                            producer_controls.idr.store(true, Ordering::Relaxed);
                        }
                        let Some(next) = seq.checked_add(1) else {
                            return;
                        };
                        seq = next;
                    }
                    None => return,
                }
            }
        })
    };

    // Pacing: sample path counters + deadline misses into the controller,
    // which writes the bitrate the producer reads each frame.
    {
        let conn = conn.clone();
        let bitrate = Arc::clone(&controls.bitrate);
        let requested = Arc::clone(&controls.requested);
        let misses = Arc::clone(&controls.deadline_misses);
        let mut controller = BitrateController::new(4_000_000, ceiling);
        workers.spawn(async move {
            let mut tick = tokio::time::interval(PACING_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                // A viewer-filed target survives past one tick: the
                // controller is steered to it, then adaptation resumes.
                let req = requested.swap(0, Ordering::Relaxed);
                if req != 0 {
                    controller.steer(req);
                }
                let missed = misses.swap(0, Ordering::Relaxed);
                let previous = controller.current();
                let path = conn.current_path_stats();
                let bps = controller.step(path, missed);
                if bps != previous {
                    tracing::debug!(
                        previous_bps = previous,
                        bitrate_bps = bps,
                        deadline_misses = missed,
                        path_rtt_ms = ?path.map(|p| p.rtt.as_millis()),
                        path_id = ?path.map(|p| p.path_id),
                        path_via_relay = ?path.map(|p| p.via_relay),
                        path_sent = ?path.map(|p| p.sent),
                        path_lost = ?path.map(|p| p.lost),
                        path_congestion_events = ?path.map(|p| p.congestion_events),
                        "desktop bitrate adapted"
                    );
                }
                bitrate.store(bps.min(u64::from(u32::MAX)), Ordering::Relaxed);
            }
        })
    };

    // Writer task: one uni stream per frame. Backpressure retains references
    // through the bounded producer queue. A broken chain
    // requests an IDR locally instead of waiting for a client roundtrip.
    let writer_conn = conn.clone();
    let writer_clock = clock.clone();
    let writer_bitrate = Arc::clone(&controls.bitrate);
    let writer_idr = Arc::clone(&controls.idr);
    let frame_route = config.frame_route.unwrap_or(rds_core::UniHello::Desktop);
    workers.spawn(async move {
        // Token bucket on the paced bitrate: offering faster than the
        // path sustains only backlogs QUIC's send buffer with frames
        // that arrive stale. Debt is capped at half a second so a large
        // keyframe can't stall the writer.
        let mut budget = 0.0f64;
        let mut last = Instant::now();
        let mut pending: Option<AdmittedFrame> = None;
        let mut chain = FrameChain::default();
        let mut sent = 0u64;
        let mut superseded = 0u64;
        let mut acknowledgements = JoinSet::new();
        let mut acknowledged = 0u64;
        let mut failed_delivery = 0u64;
        let mut health = Instant::now();
        'writer: loop {
            while let Some(result) = acknowledgements.try_join_next() {
                if matches!(result, Ok(true)) {
                    acknowledged += 1;
                } else {
                    failed_delivery += 1;
                    chain.next = None;
                    writer_idr.store(true, Ordering::Relaxed);
                }
            }
            if acknowledgements.len() >= MAX_PENDING_FRAME_ACKS {
                if matches!(acknowledgements.join_next().await, Some(Ok(true))) {
                    acknowledged += 1;
                } else {
                    failed_delivery += 1;
                    chain.next = None;
                    writer_idr.store(true, Ordering::Relaxed);
                }
                continue;
            }
            let mut produced = match pending.take() {
                Some(p) => p,
                None => match rx.recv().await {
                    Some(p) => p,
                    None => break,
                },
            };
            if produced.payload.is_empty() {
                chain.next = None;
                writer_idr.store(true, Ordering::Relaxed);
                continue;
            }
            let bps = writer_bitrate.load(Ordering::Relaxed).max(50_000) as f64 / 8.0;
            let now = Instant::now();
            budget = (budget + now.duration_since(last).as_secs_f64() * bps).min(bps * 0.25);
            last = now;
            let cost = produced.payload.len() as f64 + 64.0;
            if cost > budget {
                let wait = ((cost - budget) / bps).min(0.5);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                budget = (budget - cost).max(-bps * 0.5);
            } else {
                budget -= cost;
            }
            // Admission follows the final selection: advancing this before
            // the pacing wait would lose track of frames collapsed afterward.
            if !chain.admit(&produced) {
                writer_idr.store(true, Ordering::Relaxed);
                continue;
            }
            produced.header.send_ts_ms = writer_clock.now_ms();
            tracing::trace!(
                frame_seq = produced.header.seq,
                keyframe = produced.header.keyframe,
                payload_bytes = produced.payload.len(),
                bitrate_bps = writer_bitrate.load(Ordering::Relaxed),
                capture_encode_ms = produced
                    .header
                    .encode_done_ts_ms
                    .saturating_sub(produced.header.capture_ts_ms),
                encode_to_send_ms = produced
                    .header
                    .send_ts_ms
                    .saturating_sub(produced.header.encode_done_ts_ms),
                "desktop frame sending"
            );
            match send_frame(
                &writer_conn,
                frame_route,
                produced,
                &mut rx,
                &mut acknowledgements,
                keyframe_pending.clone(),
                writer_idr.clone(),
            )
            .await
            {
                SendOutcome::Sent => {
                    sent += 1;
                }
                SendOutcome::Superseded(newer) => {
                    superseded += 1;
                    pending = Some(newer);
                }
                SendOutcome::Done => {
                    while acknowledgements.join_next().await.is_some() {}
                    break 'writer;
                }
                SendOutcome::Failed => {
                    tracing::warn!(sent, superseded, "desktop frame writer ended");
                    break 'writer;
                }
            }
            if health.elapsed() >= Duration::from_secs(5) {
                tracing::info!(
                    sent,
                    superseded,
                    acknowledged,
                    failed_delivery,
                    pending_acknowledgements = acknowledgements.len(),
                    pending_media_frames = capture_admission.load(Ordering::Acquire),
                    keyframe_pending = keyframe_pending.load(Ordering::Acquire),
                    bitrate_bps = writer_bitrate.load(Ordering::Relaxed),
                    "desktop sender health"
                );
                health = Instant::now();
            }
        }
    });

    // Control loop: input + encoder steering + heartbeat, until the
    // peer goes away. `send` also carries DesktopEvent replies.
    let send_clock = clock.clone();
    let session_display = hello.display;
    let control = async {
        let mut assembly = crate::clipboard::Assembly::default();
        let mut clipboard = None;
        loop {
            match read_frame::<_, DesktopControl>(&mut recv).await {
                Ok(DesktopControl::Input(ev)) => {
                    if config.view_only {
                        tracing::debug!("view-only input dropped");
                        continue;
                    }
                    // The grant/display constraint was scoped to the
                    // hello's display — an event targeting another
                    // display is out of scope. Skip it (and don't ack:
                    // an ack reports the event handled).
                    if ev.display_id != session_display {
                        tracing::warn!(
                            event_display = ev.display_id,
                            session_display,
                            "input event for out-of-scope display dropped"
                        );
                        continue;
                    }
                    let mut worker = input.take().unwrap_or_else(|| {
                        super::input::worker::InputWorker::new(input_sink.take())
                    });
                    let seq = ev.seq;
                    match tokio::time::timeout(FRAME_SEND_TIMEOUT, worker.inject(ev)).await {
                        Ok(Ok(())) => input = Some(worker),
                        Ok(Err(e)) => {
                            input = Some(worker);
                            tracing::warn!("input injection failed: {e}");
                            continue;
                        }
                        Err(_) => {
                            // The platform input call never returned (a
                            // wedged X server). Drop the worker — its
                            // running syscall may still finish, per its
                            // contract — so the next event probes a fresh
                            // sink, and count this event unacked rather
                            // than stalling the whole control plane.
                            tracing::warn!("input injection timed out; dropping wedged worker");
                            continue;
                        }
                    }
                    if acks {
                        let ack = DesktopEvent::InputAck {
                            seq,
                            handled_ts_ms: send_clock.now_ms(),
                        };
                        if !matches!(
                            tokio::time::timeout(
                                FRAME_SEND_TIMEOUT,
                                write_frame(&mut send.0, &ack)
                            )
                            .await,
                            Ok(Ok(()))
                        ) {
                            break;
                        }
                    }
                }
                Ok(DesktopControl::RequestIdr) => {
                    controls.idr.store(true, Ordering::Relaxed);
                }
                Ok(DesktopControl::SetBitrate(bps)) => {
                    let bps = u64::from(bps.max(50_000)).min(ceiling);
                    controls.requested.store(bps, Ordering::Relaxed);
                }
                Ok(DesktopControl::Heartbeat { seq, ts_ms }) => {
                    if !matches!(
                        tokio::time::timeout(
                            FRAME_SEND_TIMEOUT,
                            write_frame(&mut send.0, &DesktopEvent::Heartbeat { seq, ts_ms })
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
                Ok(DesktopControl::ClipboardChunk {
                    id,
                    offset,
                    total,
                    data,
                }) => {
                    if config.view_only {
                        tracing::warn!("view-only clipboard refused");
                        break;
                    }
                    let text = match assembly.push(id, offset, total, data) {
                        Ok(text) => text,
                        Err(error) => {
                            tracing::warn!(%error,"clipboard transfer refused");
                            break;
                        }
                    };
                    if let Some(text) = text {
                        let owner = clipboard
                            .get_or_insert_with(|| crate::clipboard::Worker::new(session_display));
                        match tokio::time::timeout(Duration::from_secs(2), owner.publish(text))
                            .await
                        {
                            Ok(Ok(())) => {
                                if write_frame(
                                    &mut send.0,
                                    &DesktopEvent::ClipboardReady { id, bytes: total },
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                            }
                            result => {
                                tracing::warn!(error=?result,"clipboard publication failed; ending control before paste input");
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(%error,"desktop control ended");
                    break;
                }
            }
        }
    };

    let result = tokio::select! {
        _ = control => Ok(()),
        res = workers.join_next() => match res {
            Some(Ok(())) | None => Ok(()),
            Some(Err(e)) => Err(DesktopError::Io(std::io::Error::other(e.to_string()))),
        },
    };
    // Normal exit joins the asynchronous siblings. Cancellation during this
    // shutdown still drops the JoinSet and aborts its remaining children.
    workers.shutdown().await;
    capture.abort_all();
    result
}

/// A canceled control reply must not end with a partial, apparently clean FIN.
struct SessionSend(SendStream);

impl Drop for SessionSend {
    fn drop(&mut self) {
        let _ = self.0.reset(0u32.into());
    }
}

/// Platform capture producer, or a `NullProducer` when the build has no
/// capture backend (session still serves control input + heartbeats).
fn platform_producer(
    _display: u32,
    _interval: Duration,
    _height: Option<u32>,
) -> Box<dyn FrameProducer> {
    #[cfg(all(target_os = "linux", feature = "x11"))]
    match x11::X11Producer::new(_display, _interval, _height) {
        Ok(p) => return Box::new(p),
        Err(e) => tracing::warn!("capture init failed: {e}"),
    }
    Box::new(NullProducer)
}

/// Producer that yields nothing — the control plane of the session
/// stays live while the video side cleanly idles.
pub struct NullProducer;

impl FrameProducer for NullProducer {
    fn produce(&mut self, _seq: u64, _c: &ProducerControls, _t: &SessionClock) -> Option<Produced> {
        None
    }
}

/// Deterministic synthetic producer for tests and benches: fixed-size
/// patterned frames at a precise cadence, honoring bitrate (throttles
/// byte generation) and IDR requests (marks the next frame a keyframe).
/// Needs no display, codec or platform — the pressure on the pipeline
/// (queueing, priority, pacing) is identical to a real encoder.
pub struct SyntheticProducer {
    interval: Duration,
    width: u32,
    height: u32,
    frame_bytes: usize,
    keyframe_every: u64,
    next_due: Instant,
}

impl SyntheticProducer {
    /// `fps` capped by the caller's `max_fps`; `frame_bytes` sets the
    /// per-frame payload so tests control offered load directly.
    pub fn new(fps: u32, width: u32, height: u32, frame_bytes: usize) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / f64::from(fps.max(1))),
            width,
            height,
            frame_bytes,
            keyframe_every: 120,
            next_due: Instant::now(),
        }
    }

    pub fn keyframe_every(mut self, n: u64) -> Self {
        self.keyframe_every = n.max(1);
        self
    }
}

/// Idle wakeups start a new slot immediately. Scheduler jitter smaller than a
/// whole slot is not encoder starvation and must not reduce the bitrate.
fn advance_cadence(
    next_due: &mut Instant,
    interval: Duration,
    now: Instant,
    resumed: bool,
) -> bool {
    if resumed {
        *next_due = now;
        return false;
    }
    *next_due += interval;
    let lateness = now.saturating_duration_since(*next_due);
    if lateness >= interval {
        *next_due = now;
        return true;
    }
    if now > *next_due {
        *next_due = now;
    }
    false
}

impl FrameProducer for SyntheticProducer {
    fn resume_after_backpressure(&mut self) {
        self.next_due = self.next_due.max(
            Instant::now()
                .checked_sub(self.interval)
                .unwrap_or_else(Instant::now),
        );
    }
    fn produce(
        &mut self,
        seq: u64,
        controls: &ProducerControls,
        clock: &SessionClock,
    ) -> Option<Produced> {
        if advance_cadence(&mut self.next_due, self.interval, Instant::now(), false) {
            controls.deadline_misses.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(sleep) = self.next_due.checked_duration_since(Instant::now()) {
            std::thread::sleep(sleep);
        }
        let capture_ts_ms = clock.now_ms();
        // Synthetic encode: bitrate hint throttles frame size (clamped),
        // IDR requests flip the next frame to keyframe.
        let wants_idr = controls.idr.swap(false, Ordering::Relaxed);
        let keyframe = wants_idr || seq.is_multiple_of(self.keyframe_every);
        let bps = controls.bitrate.load(Ordering::Relaxed);
        let cap = (bps / 8 / 30).max(256) as usize; // ~30fps byte budget floor
        let len = self.frame_bytes.min(cap.max(self.frame_bytes.min(256)));
        let mut payload = vec![0u8; len.max(64)];
        payload[..8].copy_from_slice(&seq.to_le_bytes());
        let encode_done_ts_ms = clock.now_ms();
        Some(Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms,
                encode_done_ts_ms,
                send_ts_ms: 0, // stamped by the writer
                keyframe,
                codec: rds_core::Codec::H264,
                width: self.width,
                height: self.height,
            },
            payload: Bytes::from(payload),
        })
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
mod x11 {
    use super::*;
    use crate::capture::x11::X11Capturer;
    use crate::codec::openh264::H264Encoder;
    use crate::{Capturer, Encoder};

    /// Live X11 capture → OpenH264 encode at a fixed cadence.
    pub struct X11Producer {
        capturer: X11Capturer,
        encoder: H264Encoder,
        interval: Duration,
        next_due: Instant,
        output_height: Option<u32>,
        last_work: Duration,
        skipped: bool,
    }

    impl X11Producer {
        pub fn new(
            display: u32,
            interval: Duration,
            requested: Option<u32>,
        ) -> Result<Self, DesktopError> {
            let output_height = match requested {
                Some(0) => None,
                Some(height) => Some(height),
                None => match std::env::var("RDS_DESKTOP_OUTPUT_HEIGHT") {
                    Ok(value) => Some(
                        value
                            .parse::<u32>()
                            .ok()
                            .filter(|height| (16..=4320).contains(height))
                            .ok_or_else(|| {
                                DesktopError::Capture(
                                    "RDS_DESKTOP_OUTPUT_HEIGHT must be 16..=4320".into(),
                                )
                            })?,
                    ),
                    Err(std::env::VarError::NotPresent) => None,
                    Err(_) => {
                        return Err(DesktopError::Capture(
                            "RDS_DESKTOP_OUTPUT_HEIGHT must be valid Unicode".into(),
                        ));
                    }
                },
            };
            let capturer = X11Capturer::new(display)?;
            let fps = 1.0 / interval.as_secs_f32();
            let encoder = H264Encoder::new(4_000_000, fps)?;
            Ok(Self {
                capturer,
                encoder,
                interval,
                next_due: Instant::now(),
                output_height,
                last_work: Duration::ZERO,
                skipped: false,
            })
        }
    }

    impl X11Producer {
        /// Damage-aware pause: while the screen is still, capture and
        /// encode cost nothing. Polls every `IDLE_POLL` for a damage
        /// event, bounded by `IDLE_MAX` so the stream still emits a
        /// refresh frame about once a second and session teardown
        /// (the caller's `is_closed` check) is never deferred past it.
        /// `idr` wakes the loop early: a viewer joining or recovering
        /// from loss asks for a keyframe and must not wait out the cap.
        fn idle_wait(&mut self, idr: &AtomicBool) -> bool {
            const IDLE_POLL: Duration = Duration::from_millis(10);
            const IDLE_MAX: Duration = Duration::from_secs(1);
            if self.capturer.changed() || idr.load(Ordering::Relaxed) {
                return false;
            }
            let deadline = Instant::now() + IDLE_MAX;
            loop {
                if self.capturer.wait_for_change(IDLE_POLL)
                    || idr.load(Ordering::Relaxed)
                    || Instant::now() >= deadline
                {
                    break;
                }
            }
            true
        }
    }

    impl FrameProducer for X11Producer {
        fn resume_after_backpressure(&mut self) {
            let interval = self
                .interval
                .max(self.last_work)
                .min(Duration::from_millis(500));
            self.next_due = self.next_due.max(
                Instant::now()
                    .checked_sub(interval)
                    .unwrap_or_else(Instant::now),
            );
        }
        fn preserves_reference(&self) -> bool {
            self.skipped
        }
        fn produce(
            &mut self,
            seq: u64,
            controls: &ProducerControls,
            clock: &SessionClock,
        ) -> Option<Produced> {
            let resumed = self.idle_wait(&controls.idr);
            // Capture/conversion has a fixed CPU cost that lowering encoded
            // bitrate cannot remove. Honor max_fps while using the achievable
            // cadence, rather than collapsing bitrate for every slow frame.
            let interval = self.interval.max(self.last_work);
            if advance_cadence(&mut self.next_due, interval, Instant::now(), resumed) {
                controls.deadline_misses.fetch_add(1, Ordering::Relaxed);
            }
            if let Some(sleep) = self.next_due.checked_duration_since(Instant::now()) {
                std::thread::sleep(sleep);
            }
            let capture_ts_ms = clock.now_ms();
            self.skipped = false;
            let work_started = Instant::now();
            let raw = match self.capturer.capture() {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("capture failed: {e}");
                    return None;
                }
            };
            let raw = match crate::scaling::downscale(raw, self.output_height) {
                Ok(raw) => raw,
                Err(error) => {
                    tracing::warn!(%error,"capture scaling failed");
                    return None;
                }
            };
            if controls.idr.swap(false, Ordering::Relaxed) {
                self.encoder.request_idr();
            }
            self.encoder.set_bitrate(
                controls
                    .bitrate
                    .load(Ordering::Relaxed)
                    .min(u64::from(u32::MAX)) as u32,
            );
            let (width, height) = (raw.width, raw.height);
            let encoded = self.encoder.encode(&raw);
            self.skipped = encoded.as_ref().is_ok_and(|frame| frame.data.is_empty());
            self.last_work = work_started.elapsed();
            match encoded {
                Ok(frame) => Some(Produced {
                    header: FrameHeader {
                        seq,
                        capture_ts_ms,
                        encode_done_ts_ms: clock.now_ms(),
                        send_ts_ms: 0,
                        keyframe: frame.keyframe,
                        codec: frame.codec,
                        width,
                        height,
                    },
                    payload: frame.data,
                }),
                Err(e) => {
                    tracing::warn!("encode failed: {e}");
                    Some(Produced {
                        header: FrameHeader {
                            seq,
                            capture_ts_ms,
                            encode_done_ts_ms: clock.now_ms(),
                            send_ts_ms: 0,
                            keyframe: false,
                            codec: rds_core::Codec::H264,
                            width,
                            height,
                        },
                        payload: Bytes::new(),
                    })
                }
            }
        }
    }
}

/// Conservative reference contract: every delta may depend on its predecessor.
/// A producer must identify independent keyframes from the encoded bitstream.
#[derive(Default)]
struct FrameChain {
    next: Option<u64>,
}

impl FrameChain {
    fn admit(&mut self, produced: &Produced) -> bool {
        let header = &produced.header;
        if produced.payload.is_empty()
            || header.seq == u64::MAX
            || (!header.keyframe && self.next != Some(header.seq))
        {
            self.next = None;
            return false;
        }
        self.next = header.seq.checked_add(1);
        true
    }
}

/// Reset code for a frame stream abandoned mid-send — the frame went
/// stale while still in flight, so its tail is dropped instead of
/// consuming path capacity the fresher frame needs.
const STALE_FRAME_RESET: u32 = 0x1;
/// One budget covers stream credit, tag, header and the complete payload.
const FRAME_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// An interrupted frame must never look like a successfully finished payload.
struct FrameSend {
    stream: SendStream,
    finished: bool,
}

impl Drop for FrameSend {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.stream.reset(STALE_FRAME_RESET.into());
        }
    }
}

/// How one frame send ended.
enum SendOutcome {
    /// Frame fully sent.
    Sent,
    /// A fresher decodable frame supersedes — send it next.
    Superseded(AdmittedFrame),
    /// Producer closed mid-send; the final frame was finished.
    Done,
    /// Transport failure — the writer ends.
    Failed,
}

/// Send one frame on its own tagged uni stream, aborting mid-write if
/// an independent keyframe lands. Otherwise finish the reference on which
/// the next delta may depend, retaining partial-write progress.
async fn send_frame(
    conn: &Connection,
    route: rds_core::UniHello,
    produced: AdmittedFrame,
    rx: &mut mpsc::Receiver<AdmittedFrame>,
    acknowledgements: &mut JoinSet<bool>,
    keyframe_pending: Arc<AtomicBool>,
    idr: Arc<AtomicBool>,
) -> SendOutcome {
    match tokio::time::timeout(
        FRAME_SEND_TIMEOUT,
        send_frame_inner(
            conn,
            route,
            produced,
            rx,
            acknowledgements,
            keyframe_pending,
            idr,
        ),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::debug!("frame send deadline exceeded");
            SendOutcome::Failed
        }
    }
}

async fn send_frame_inner(
    conn: &Connection,
    route: rds_core::UniHello,
    produced: AdmittedFrame,
    rx: &mut mpsc::Receiver<AdmittedFrame>,
    acknowledgements: &mut JoinSet<bool>,
    keyframe_pending: Arc<AtomicBool>,
    idr: Arc<AtomicBool>,
) -> SendOutcome {
    let AdmittedFrame { produced, permit } = produced;
    let mut sending = match conn.open_uni().await {
        Ok(stream) => FrameSend {
            stream,
            finished: false,
        },
        Err(e) => {
            tracing::debug!("frame stream open failed: {e}");
            return SendOutcome::Failed;
        }
    };
    let stream = &mut sending.stream;
    // Frame streams rank below the control stream — a stale frame
    // must never delay an input event or a resync request.
    if let Err(e) = stream.set_priority(MEDIA_STREAM_PRIORITY) {
        tracing::debug!("frame stream priority failed: {e}");
    }
    // Every uni stream leads with its UniHello tag — the receiver's
    // per-connection demux routes on it. Per-session routes keep a stale
    // stream out of any replacement session's inbox.
    if let Err(e) = write_frame(&mut *stream, &route).await {
        tracing::debug!("frame tag write failed: {e}");
        return SendOutcome::Failed;
    }
    if let Err(e) = write_frame(&mut *stream, &produced.header).await {
        tracing::debug!("frame header write failed: {e}");
        return SendOutcome::Failed;
    }
    let outcome = match send_payload(stream, &produced, rx).await {
        Ok(PayloadOutcome::Abandoned(next)) => {
            return SendOutcome::Superseded(next);
        }
        Ok(PayloadOutcome::Sent) => SendOutcome::Sent,
        Ok(PayloadOutcome::Superseded(next)) => SendOutcome::Superseded(next),
        Ok(PayloadOutcome::ProducerEnded) => SendOutcome::Done,
        Err(e) => {
            tracing::debug!("frame send failed: {e}");
            return SendOutcome::Failed;
        }
    };
    if let Err(e) = stream.finish() {
        tracing::debug!("frame finish failed: {e}");
        return SendOutcome::Failed;
    }
    let seq = produced.header.seq;
    let keyframe = produced.header.keyframe;
    let payload_bytes = produced.payload.len();
    // Retain the reset-on-drop owner until delivery is acknowledged. The
    // bounded task group is owned by this writer; cancellation resets its
    // outstanding frames without closing unrelated connection services.
    acknowledgements.spawn(async move {
        let started = Instant::now();
        let acknowledged = match tokio::time::timeout(FRAME_ACK_TIMEOUT, sending.stream.stopped()).await {
            Ok(Ok(None)) => {
                sending.finished = true;
                tracing::trace!(frame_seq=seq,payload_bytes,ack_ms=started.elapsed().as_millis(),"desktop frame transport acknowledged");
                true
            }
            result => {
                idr.store(true, Ordering::Relaxed);
                tracing::warn!(frame_seq=seq,payload_bytes,ack_ms=started.elapsed().as_millis(),outcome=?result,"desktop frame delivery unconfirmed");
                false
            }
        };
        // Capture the complete reset owner, including its Drop implementation,
        // rather than allowing disjoint field captures in the async closure.
        drop(sending);
        drop(permit);
        if keyframe {
            keyframe_pending.store(false, Ordering::Release);
        }
        acknowledged
    });
    outcome
}

/// Completed payloads may FIN; abandoned ones must RESET.
enum PayloadOutcome<T> {
    Sent,
    Superseded(T),
    ProducerEnded,
    Abandoned(T),
}

async fn send_payload<W: AsyncWrite + Unpin, T: Borrow<Produced>>(
    stream: &mut W,
    produced: &Produced,
    rx: &mut mpsc::Receiver<T>,
) -> std::io::Result<PayloadOutcome<T>> {
    // write_all is not cancellation-safe: keep its progress alive across a
    // producer event. Starting another write_all would duplicate the prefix.
    let writing = stream.write_all(&produced.payload);
    tokio::pin!(writing);
    tokio::select! {
        result = &mut writing => {
            result?;
            Ok(PayloadOutcome::Sent)
        }
        newer = rx.recv() => match newer {
            None => {
                writing.await?;
                Ok(PayloadOutcome::ProducerEnded)
            }
            Some(newer) if produced.header.keyframe || !newer.borrow().header.keyframe => {
                writing.await?;
                Ok(PayloadOutcome::Superseded(newer))
            }
            Some(newer) => Ok(PayloadOutcome::Abandoned(newer)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_wakes_and_subslot_jitter_do_not_collapse_bitrate() {
        let interval = Duration::from_millis(16);
        let mut due = Instant::now();
        let mut controller = BitrateController::new(4_000_000, 8_000_000);
        for _ in 0..64 {
            let now = due + Duration::from_secs(1);
            let missed = advance_cadence(&mut due, interval, now, true);
            assert_eq!(due, now, "damage wake must capture immediately");
            assert!(!missed, "idle wake must not count as encoder starvation");
            controller.step(None, u64::from(missed));
            let now = due + interval + Duration::from_micros(100);
            assert!(!advance_cadence(&mut due, interval, now, false));
        }
        assert_eq!(controller.current(), 4_000_000);
        let now = due + interval * 3;
        assert!(advance_cadence(&mut due, interval, now, false));
        assert_eq!(due, now, "real starvation starts a fresh schedule");
        assert!(controller.step(None, 1) < 4_000_000);
    }

    #[test]
    fn software_work_slower_than_max_fps_preserves_network_bitrate() {
        let maximum_fps_interval = Duration::from_millis(16);
        let mut due = Instant::now();
        let mut controller = BitrateController::new(4_000_000, 8_000_000);
        for work in [22, 24, 55, 3].into_iter().cycle().take(64) {
            let work = Duration::from_millis(work);
            let now = due + work;
            let missed = advance_cadence(&mut due, maximum_fps_interval.max(work), now, false);
            assert!(
                !missed,
                "known capture/codec cost is an achievable cadence, not network congestion"
            );
            assert!(due >= now);
            controller.step(None, u64::from(missed));
        }
        assert_eq!(controller.current(), 4_000_000);
    }

    #[test]
    fn admission_pause_does_not_report_encoder_starvation() {
        let controls = ProducerControls::new(4_000_000);
        let mut source = SyntheticProducer::new(60, 64, 64, 256);
        source.next_due = Instant::now() - Duration::from_secs(2);
        source.resume_after_backpressure();
        assert!(
            source
                .produce(0, &controls, &SessionClock::default())
                .is_some()
        );
        assert_eq!(controls.deadline_misses.load(Ordering::Relaxed), 0);
        // Real lateness without a deliberate admission pause still reports.
        source.next_due = Instant::now() - Duration::from_secs(2);
        source.produce(1, &controls, &SessionClock::default());
        assert_eq!(controls.deadline_misses.load(Ordering::Relaxed), 1);
    }

    fn produced(seq: u64, keyframe: bool) -> Produced {
        Produced {
            header: FrameHeader {
                seq,
                capture_ts_ms: 0,
                encode_done_ts_ms: 0,
                send_ts_ms: 0,
                keyframe,
                codec: rds_core::Codec::H264,
                width: 16,
                height: 16,
            },
            payload: Bytes::from((0..4096).map(|n| (n % 251) as u8).collect::<Vec<_>>()),
        }
    }

    async fn partial_write(closed: bool, keyframe: bool) {
        use std::future::{Future, poll_fn};
        use std::task::Poll;
        use tokio::io::AsyncReadExt;

        tokio::time::timeout(Duration::from_secs(3), async {
            let (mut writer, mut reader) = tokio::io::duplex(64);
            let (tx, mut rx) = mpsc::channel(1);
            let frame = produced(0, keyframe);
            let mut wire = vec![0; 17];
            {
                let mut sending = Box::pin(send_payload(&mut writer, &frame, &mut rx));
                // Poll until the tiny stream buffer is full, then consume only
                // a prefix. The producer event must interrupt a partial write.
                poll_fn(|cx| {
                    assert!(sending.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                }).await;
                reader.read_exact(&mut wire).await.unwrap();
                if !closed {
                    tx.send(produced(1, false)).await.unwrap();
                }
                drop(tx);
                poll_fn(|cx| {
                    assert!(sending.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                }).await;
                let (result, ()) = tokio::join!(sending, async {
                    // Drain enough to complete the write; after dropping the
                    // writer below, read to EOF to detect any extra bytes.
                    while wire.len() < frame.payload.len() {
                        let mut chunk = [0; 256];
                        let n = reader.read(&mut chunk).await.unwrap();
                        assert_ne!(n, 0);
                        wire.extend_from_slice(&chunk[..n]);
                    }
                });
                if closed {
                    assert!(matches!(result.unwrap(), PayloadOutcome::ProducerEnded));
                } else {
                    assert!(matches!(result.unwrap(), PayloadOutcome::Superseded(next) if next.header.seq == 1));
                }
            }
            drop(writer);
            reader.read_to_end(&mut wire).await.unwrap();
            assert_eq!(wire.len(), frame.payload.len(), "partial-write prefix was duplicated");
            assert_eq!(wire.as_slice(), frame.payload.as_ref());
        }).await.expect("partial frame send hung");
    }

    #[tokio::test]
    async fn partial_keyframe_supersession_preserves_exact_bytes() {
        partial_write(false, true).await;
    }

    #[tokio::test]
    async fn partial_final_frame_preserves_exact_bytes() {
        partial_write(true, true).await;
        partial_write(true, false).await;
    }

    #[tokio::test]
    async fn partial_delta_finishes_before_its_dependent_successor() {
        partial_write(false, false).await;
    }

    #[tokio::test]
    async fn interrupted_payload_preserves_write_errors() {
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        for closed in [false, true] {
            let (mut writer, reader) = tokio::io::duplex(64);
            let (tx, mut rx) = mpsc::channel(1);
            let frame = produced(0, true);
            let mut sending = Box::pin(send_payload(&mut writer, &frame, &mut rx));
            poll_fn(|cx| {
                assert!(sending.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            if !closed {
                tx.send(produced(1, true)).await.unwrap();
            }
            drop(tx);
            // Select the producer event before causing the write to fail.
            poll_fn(|cx| {
                assert!(sending.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(reader);
            assert!(
                tokio::time::timeout(Duration::from_secs(1), sending)
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn independent_keyframe_replaces_delta_without_waiting_for_peer() {
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        {
            let (mut writer, _reader) = tokio::io::duplex(64);
            let (tx, mut rx) = mpsc::channel(1);
            let frame = produced(0, false);
            let mut sending = Box::pin(send_payload(&mut writer, &frame, &mut rx));
            poll_fn(|cx| {
                assert!(sending.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            tx.send(produced(1, true)).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), sending)
                .await
                .unwrap()
                .unwrap();
            let PayloadOutcome::Abandoned(next) = result else {
                panic!("stale delta was not abandoned");
            };
            assert_eq!(next.header.seq, 1);
            assert!(next.header.keyframe);
        }
    }

    #[test]
    fn reference_chain_waits_for_keyframe_after_gap_empty_or_sequence_end() {
        let mut chain = FrameChain::default();
        assert!(!chain.admit(&produced(0, false)));
        assert!(chain.admit(&produced(1, true)));
        assert!(chain.admit(&produced(2, false)));
        assert!(!chain.admit(&produced(4, false))); // Missing reference 3.
        assert!(!chain.admit(&produced(5, false)));
        assert!(chain.admit(&produced(6, true)));
        let mut empty = produced(7, false);
        empty.payload = Bytes::new();
        assert!(!chain.admit(&empty));
        assert!(!chain.admit(&produced(8, false)));
        assert!(chain.admit(&produced(u64::MAX - 1, true)));
        assert!(!chain.admit(&produced(u64::MAX, false)));
        assert!(!chain.admit(&produced(0, false)));
    }

    #[cfg(feature = "x11")]
    #[test]
    fn native_h264_chain_recovers_after_a_lost_reference() {
        use crate::{Decoder, EncodedFrame, Encoder, H264Decoder, H264Encoder, RawFrame};

        let mut encoder = H264Encoder::new(4_000_000, 30.0).unwrap();
        let mut decoder = H264Decoder::new().unwrap();
        let mut chain = FrameChain::default();
        let mut decoded = Vec::new();
        for seq in 0..7 {
            let mut bgra = vec![0; 64 * 64 * 4];
            for (i, pixel) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let value = if (i / 64 + seq as usize * 4) % 32 < 16 {
                    220
                } else {
                    40
                };
                pixel.copy_from_slice(&[value, value, value, 255]);
            }
            if seq == 5 {
                encoder.request_idr();
            }
            let encoded = encoder
                .encode(&RawFrame {
                    width: 64,
                    height: 64,
                    stride: 256,
                    data: bgra.into(),
                })
                .unwrap();
            assert!(!encoded.data.is_empty());
            assert_eq!(encoded.keyframe, seq == 0 || seq == 5);
            let mut frame = produced(seq, encoded.keyframe);
            frame.header.width = 64;
            frame.header.height = 64;
            frame.payload = encoded.data;
            if seq == 2 {
                // Simulate an encoded frame lost to backpressure.
                continue;
            }
            if chain.admit(&frame) {
                let raw = decoder
                    .decode(&EncodedFrame {
                        codec: frame.header.codec,
                        keyframe: frame.header.keyframe,
                        data: frame.payload,
                    })
                    .unwrap()
                    .expect("admitted H.264 frame must decode");
                assert_eq!((raw.width, raw.height), (64, 64));
                // Check a native decoded pixel after recovery, not just a header.
                let expected = if (seq * 4) % 32 < 16 { 220i16 } else { 40 };
                assert!((i16::from(raw.data[0]) - expected).abs() < 12);
                decoded.push(seq);
            }
        }
        assert_eq!(decoded, [0, 1, 5, 6]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_frame_writer_resets_stream_and_preserves_connection() {
        for backend in [rds_net::Backend::Iroh, rds_net::Backend::Noq] {
            tokio::time::timeout(Duration::from_secs(10), async {
                let config = rds_net::EndpointConfig {
                    backend,
                    discovery: false,
                    bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                    ..Default::default()
                };
                let client = rds_net::bind_endpoint(config.clone()).await.unwrap();
                let server = rds_net::bind_endpoint(config).await.unwrap();
                let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
                    server.accept().await.unwrap().await
                });
                let (a, b) = (a.unwrap(), b.unwrap());
                let stream = a.open_uni().await.unwrap();
                let writer = tokio::spawn(async move {
                    let mut sending = FrameSend {
                        stream,
                        finished: false,
                    };
                    sending.stream.write_all(b"prefix").await.unwrap();
                    std::future::pending::<()>().await;
                });
                let mut recv = b.accept_uni().await.unwrap();
                let mut prefix = [0; 6];
                recv.read_exact(&mut prefix).await.unwrap();
                assert_eq!(&prefix, b"prefix");
                writer.abort();
                assert!(writer.await.unwrap_err().is_cancelled());
                assert!(matches!(recv.read(&mut prefix).await,
                    Err(rds_net::ReadError::Reset(code)) if code == STALE_FRAME_RESET.into()));

                // Reset belongs to the abandoned frame, not the connection.
                let mut next = a.open_uni().await.unwrap();
                next.write_all(b"usable").await.unwrap();
                next.finish().unwrap();
                let mut recv = b.accept_uni().await.unwrap();
                assert_eq!(recv.read_to_end(6).await.unwrap(), b"usable");
                a.close(0u32.into(), b"done");
                client.close().await;
                server.close().await;
            })
            .await
            .expect("frame cancellation did not release transport resources");
        }
    }

    fn path(sent: u64, lost: u64, rtt_ms: u64, congestion: u64) -> PathStats {
        PathStats {
            path_id: 0,
            rtt: Duration::from_millis(rtt_ms),
            cwnd: 64 * 1024,
            sent,
            lost,
            sent_bytes: sent * 1200,
            recv_bytes: 0,
            congestion_events: congestion,
            selected: true,
            via_relay: false,
            relay_slot: None,
        }
    }

    #[test]
    fn controller_drops_bitrate_on_loss() {
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        c.step(Some(path(1000, 0, 20, 0)), 0); // prime baseline
        let bps = c.step(Some(path(2000, 80, 20, 0)), 0); // 4% loss
        assert!(bps < 4_000_000, "loss must cut bitrate, got {bps}");
    }

    #[test]
    fn controller_recovers_toward_ceiling() {
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        c.step(Some(path(1000, 0, 20, 0)), 0);
        let mut bps = c.current();
        for i in 0..20 {
            bps = c.step(Some(path(2000 + i * 1000, 0, 20, 0)), 0);
        }
        assert_eq!(bps, 8_000_000, "clean windows must reach the ceiling");
    }

    #[test]
    fn controller_honors_floor_and_misses() {
        let mut c = BitrateController::new(200_000, 8_000_000);
        c.step(Some(path(1000, 0, 20, 0)), 0);
        for _ in 0..10 {
            c.step(Some(path(0, 0, 20, 0)), 5); // encoder starving
        }
        assert_eq!(c.current(), 100_000, "deadline misses bottom out at floor");
    }

    #[test]
    fn controller_reacts_to_rtt_growth() {
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        c.step(Some(path(1000, 0, 20, 0)), 0);
        let bps = c.step(Some(path(2000, 0, 60, 0)), 0); // 3× baseline RTT
        assert!(bps < 4_000_000, "RTT growth must cut bitrate, got {bps}");
    }

    #[test]
    fn sustained_rtt_change_without_loss_does_not_collapse_bitrate() {
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        c.step(Some(path(1000, 0, 90, 0)), 0);
        let reduced = c.step(Some(path(2000, 0, 240, 0)), 0);
        assert!(reduced < 4_000_000, "a new delay increase must react");
        for i in 0..40 {
            c.step(Some(path(3000 + i * 1000, 0, 240, 0)), 0);
        }
        assert_eq!(c.current(), 8_000_000, "stable delay is not new congestion");
        assert!(c.step(Some(path(44000, 0, 500, 0)), 0) < 8_000_000);
    }

    #[test]
    fn first_sample_never_penalizes() {
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        // A path that already lost packets before we started watching.
        let bps = c.step(Some(path(1_000_000, 500_000, 20, 0)), 0);
        assert_eq!(bps, 4_000_000, "first sample establishes baseline only");
    }

    #[test]
    fn steer_sets_target_inside_bounds() {
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        c.steer(2_500_000);
        assert_eq!(c.current(), 2_500_000);
        // Out-of-range requests clamp to the controller's own contract.
        c.steer(50_000);
        assert_eq!(c.current(), 100_000);
        c.steer(50_000_000);
        assert_eq!(c.current(), 8_000_000);
    }

    #[test]
    fn steer_survives_adaptation_ticks() {
        // The SetBitrate semantics the viewer sees: a filed target is not
        // overwritten by the next pacing step — adaptation resumes *from*
        // the steered point.
        let mut c = BitrateController::new(4_000_000, 8_000_000);
        c.step(Some(path(1000, 0, 20, 0)), 0); // prime baseline
        c.steer(1_000_000);
        let bps = c.step(Some(path(2000, 0, 20, 0)), 0);
        // Clean window: one recovery step from 1Mbps, not a snap back to 4.
        assert!(
            bps > 1_000_000 && bps < 2_000_000,
            "adaptation must resume from the steered target, got {bps}"
        );
    }
}
