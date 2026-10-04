use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use rds_core::{DesktopControl, InputEvent, InputKind};
use tokio::sync::Notify;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::PhysicalKey,
    window::{Icon, Window, WindowId, WindowLevel},
};

use super::{
    gpu::{DrawOutcome, Gpu},
    input::{Viewport, evdev, modifier_changes},
};
use crate::{DesktopError, RawFrame};

#[derive(Debug)]
pub enum ViewerInput {
    Control(DesktopControl),
    Close,
}

struct InputState {
    queue: Mutex<VecDeque<ViewerInput>>,
    ready: Notify,
    pointer_coalesced: AtomicU64,
    input_dropped: AtomicU64,
    max_depth: AtomicU64,
}
struct InputSender(Arc<InputState>);
const INPUT_QUEUE_CAPACITY: usize = 1024;

/// One bounded input consumer. Consecutive pointer positions collapse, while
/// keys/buttons retain their order relative to the final pointer before them.
/// A full queue fails closed. Motion before a button/key/scroll is a
/// position barrier, not stale data: removing it could redirect the action.
pub struct InputReceiver(Arc<InputState>);
impl InputReceiver {
    pub async fn recv(&mut self) -> Option<ViewerInput> {
        loop {
            let ready = self.0.ready.notified();
            if let Some(event) = self.try_recv() {
                return Some(event);
            }
            ready.await;
        }
    }
    pub fn try_recv(&self) -> Option<ViewerInput> {
        self.0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }
}
impl InputSender {
    fn send(&self, message: ViewerInput) -> Result<(), ()> {
        let mut queue = self
            .0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let (
            ViewerInput::Control(DesktopControl::Input(next)),
            Some(ViewerInput::Control(DesktopControl::Input(previous))),
        ) = (&message, queue.back())
            && next.display_id == previous.display_id
            && matches!(next.kind, InputKind::PointerMove { .. })
            && matches!(previous.kind, InputKind::PointerMove { .. })
        {
            queue.pop_back();
            self.0.pointer_coalesced.fetch_add(1, Ordering::Relaxed);
        }
        if queue.len() >= INPUT_QUEUE_CAPACITY {
            self.0.input_dropped.fetch_add(1, Ordering::Relaxed);
            return Err(());
        }
        queue.push_back(message);
        self.0
            .max_depth
            .fetch_max(queue.len() as u64, Ordering::Relaxed);
        drop(queue);
        self.0.ready.notify_one();
        Ok(())
    }
}
impl Drop for InputSender {
    fn drop(&mut self) {
        let mut queue = self
            .0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.clear();
        queue.push_back(ViewerInput::Close);
        drop(queue);
        self.0.ready.notify_one();
    }
}

#[derive(Clone, Default, Debug, serde::Serialize)]
pub struct ViewerReport {
    pub session_epoch: u64,
    pub frames_received: u64,
    pub frames_submitted: u64,
    pub gpu_uploads: u64,
    pub last_submission_ms: Option<u64>,
    pub frames_replaced: u64,
    pub redraws: u64,
    pub surface_skips: u64,
    pub first_frame_ms: Option<u64>,
    pub receive_to_submit_p50_ms: Option<f64>,
    pub receive_to_submit_p95_ms: Option<f64>,
    pub capture_encode_p50_ms: Option<f64>,
    pub capture_encode_p95_ms: Option<f64>,
    pub encode_to_send_p50_ms: Option<f64>,
    pub encode_to_send_p95_ms: Option<f64>,
    pub control_rtt_ms: Option<u64>,
    pub last_control_echo_ms: Option<u64>,
    pub input_acks: u64,
    pub inputs_dispatched: u64,
    pub input_queue_depth: usize,
    pub input_acks_canceled: u64,
    pub slow_input_acks: u64,
    pub last_input_ack_ms: Option<f64>,
    pub input_acks_matched: u64,
    pub input_acks_unmatched: u64,
    pub input_ack_tracking_evicted: u64,
    pub managed_events_separated: bool,
    pub input_ack_p50_ms: Option<f64>,
    pub input_ack_p95_ms: Option<f64>,
    pub input_queue_p95_ms: Option<f64>,
    pub input_pointer_coalesced: u64,
    pub input_events_dropped: u64,
    pub input_queue_max_depth: u64,
    pub pending_input_acks: usize,
    pub oldest_input_ack_age_ms: Option<u64>,
    pub clipboard_transfers: u64,
    pub last_clipboard_bytes: u32,
    pub reconnects: u64,
    pub video_repair_requests: u64,
    pub last_recovery_ms: Option<u64>,
    pub last_frame_ms: Option<u64>,
    pub video_width: u32,
    pub video_height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_probe: Option<super::VisualProbeReport>,
}

#[derive(serde::Serialize)]
pub struct ViewerSnapshot {
    pub pid: u32,
    pub elapsed_ms: u64,
    pub status: String,
    pub network_stage: String,
    pub render_stage: String,
    pub ui_event_age_ms: u64,
    pub decoded_frame_age_ms: Option<u64>,
    pub encoded_frame_age_ms: Option<u64>,
    pub control_echo_age_ms: Option<u64>,
    pub submission_age_ms: Option<u64>,
    pub pending_frame_bytes: usize,
    pub occluded: bool,
    pub report: ViewerReport,
}

struct Pending {
    raw: RawFrame,
    received: Instant,
    frame_seq: Option<u64>,
}
/// Local event-to-ack measurements never compare clocks on different hosts.
/// Coalesced pointer events are tracked only after dequeue; the event's own
/// local creation time still includes time spent waiting in the UI queue.
#[derive(Default)]
struct InputLatency {
    pending: VecDeque<(u64, u64)>,
    queued: Vec<f64>,
    acknowledged: Vec<f64>,
    evicted: u64,
    last_slow_log_ms: Option<u64>,
}
impl InputLatency {
    fn sent(&mut self, seq: u64, created_ms: u64, now_ms: u64) {
        if self.pending.len() == INPUT_QUEUE_CAPACITY {
            self.pending.pop_front();
            self.evicted += 1;
        }
        self.pending.push_back((seq, created_ms));
        sample(&mut self.queued, now_ms.saturating_sub(created_ms) as f64);
    }
    fn ack(&mut self, seq: u64, now_ms: u64) -> Option<f64> {
        if let Some(index) = self.pending.iter().position(|(pending, _)| *pending == seq)
            && let Some((_, created_ms)) = self.pending.remove(index)
        {
            let latency = now_ms.saturating_sub(created_ms) as f64;
            sample(&mut self.acknowledged, latency);
            return Some(latency);
        }
        None
    }
}

struct State {
    pending: Option<Pending>,
    wake_pending: bool,
    status: String,
    extent: (u32, u32),
    report: ViewerReport,
    delays: Vec<f64>,
    encoding: Vec<f64>,
    sending: Vec<f64>,
    input_latency: InputLatency,
    close: bool,
    interrupted: Option<Instant>,
    network_stage: String,
    render_stage: String,
    last_ui_ms: u64,
    last_encoded_ms: Option<u64>,
    occluded: bool,
    visual_probe: Option<super::visual_probe::VisualProbe>,
    session_span: tracing::Span,
}
fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn sample(samples: &mut Vec<f64>, value: f64) {
    if samples.len() == 1024 {
        samples.remove(0);
    }
    samples.push(value);
}
fn quantiles(samples: &[f64]) -> (Option<f64>, Option<f64>) {
    if samples.is_empty() {
        return (None, None);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    (
        Some(sorted[(sorted.len() - 1) * 50 / 100]),
        Some(sorted[(sorted.len() - 1) * 95 / 100]),
    )
}

#[derive(Clone)]
pub struct ViewerHandle {
    state: Arc<Mutex<State>>,
    input_state: Arc<InputState>,
    proxy: EventLoopProxy<()>,
    started: Instant,
}
impl ViewerHandle {
    /// An independent diagnostic tick probes UI dispatch even when no media
    /// arrives. The existing coalescing flag keeps a stalled loop bounded.
    pub fn heartbeat_ui(&self) {
        let mut state = lock(&self.state);
        self.wake(&mut state);
    }
    pub fn stage(&self, stage: &str) {
        lock(&self.state).network_stage = stage.into();
    }
    pub fn snapshot(&self) -> ViewerSnapshot {
        let report = self.report();
        let state = lock(&self.state);
        let elapsed = self.started.elapsed().as_millis() as u64;
        ViewerSnapshot {
            pid: std::process::id(),
            elapsed_ms: elapsed,
            status: state.status.clone(),
            network_stage: state.network_stage.clone(),
            render_stage: state.render_stage.clone(),
            ui_event_age_ms: elapsed.saturating_sub(state.last_ui_ms),
            decoded_frame_age_ms: report.last_frame_ms.map(|v| elapsed.saturating_sub(v)),
            encoded_frame_age_ms: state.last_encoded_ms.map(|v| elapsed.saturating_sub(v)),
            control_echo_age_ms: report
                .last_control_echo_ms
                .map(|v| elapsed.saturating_sub(v)),
            submission_age_ms: report.last_submission_ms.map(|v| elapsed.saturating_sub(v)),
            pending_frame_bytes: state.pending.as_ref().map_or(0, |p| p.raw.data.len()),
            occluded: state.occluded,
            report,
        }
    }
    fn wake(&self, state: &mut State) {
        if !state.wake_pending {
            state.wake_pending = true;
            let _ = self.proxy.send_event(());
        }
    }
    pub fn frame(&self, raw: RawFrame, received: Instant) {
        self.queue_frame(raw, received, None);
    }
    /// Preserve an encoded frame's sequence for causal stage correlation.
    pub fn frame_with_seq(&self, raw: RawFrame, received: Instant, seq: u64) {
        self.queue_frame(raw, received, Some(seq));
    }
    fn queue_frame(&self, raw: RawFrame, received: Instant, frame_seq: Option<u64>) {
        let mut state = lock(&self.state);
        state.report.frames_received += 1;
        state.report.video_width = raw.width;
        state.report.video_height = raw.height;
        state.report.last_frame_ms = Some(self.started.elapsed().as_millis() as u64);
        if let Some(interrupted) = state.interrupted.take() {
            state.report.last_recovery_ms = Some(interrupted.elapsed().as_millis() as u64);
        }
        state.status = "Connected".into();
        if state
            .pending
            .replace(Pending {
                raw,
                received,
                frame_seq,
            })
            .is_some()
        {
            state.report.frames_replaced += 1;
        }
        self.wake(&mut state);
    }
    pub fn status(&self, text: impl Into<String>) {
        let mut state = lock(&self.state);
        state.status = text.into();
        if state.status == "Reconnecting" {
            state.report.input_acks_canceled += state.input_latency.pending.len() as u64;
            state.input_latency.pending.clear();
            if let Some(probe) = &mut state.visual_probe {
                probe.reset();
            }
        }
        if state.status == "Reconnecting" && state.interrupted.is_none() {
            state.interrupted = Some(Instant::now());
            state.report.reconnects += 1;
        }
        self.wake(&mut state);
    }
    pub fn display_extent(&self, width: u32, height: u32) {
        lock(&self.state).extent = (width, height);
    }
    /// A local epoch distinguishes frame/input sequence numbers after reconnect.
    pub fn begin_session(&self, span: tracing::Span) {
        let mut state = lock(&self.state);
        state.session_span = span;
        state.report.session_epoch = state.report.session_epoch.saturating_add(1);
        tracing::info!(parent: &state.session_span,
            viewer_pid = std::process::id(),
            viewer_epoch = state.report.session_epoch,
            "native desktop attempt started"
        );
    }
    /// Configure an opt-in controlled marker; no pixels or text enter reports.
    pub fn visual_probe(&self, spec: super::VisualProbeSpec) -> Result<(), DesktopError> {
        lock(&self.state).visual_probe = Some(super::visual_probe::VisualProbe::new(spec)?);
        Ok(())
    }
    pub fn control_rtt(&self, ms: u64) {
        let mut state = lock(&self.state);
        state.report.control_rtt_ms = Some(ms);
        state.report.last_control_echo_ms = Some(self.started.elapsed().as_millis() as u64);
    }
    pub fn input_sent(&self, control: &DesktopControl) {
        if matches!(control, DesktopControl::RequestIdr) {
            self.video_repair_requested();
        }
        if let DesktopControl::Input(event) = control {
            let sent_ms = self.started.elapsed().as_millis() as u64;
            {
                let mut state = lock(&self.state);
                state.report.inputs_dispatched += 1;
                state
                    .input_latency
                    .sent(event.seq, event.event_ts_ms, sent_ms);
            }
            let event_class = match event.kind {
                InputKind::KeyDown { .. } => "key_down",
                InputKind::KeyUp { .. } => "key_up",
                InputKind::PointerMove { .. } | InputKind::PointerMotion { .. } => "pointer_move",
                InputKind::PointerButton { pressed: true, .. } => "button_down",
                InputKind::PointerButton { pressed: false, .. } => "button_up",
                InputKind::Scroll { .. } => "scroll",
            };
            let epoch = lock(&self.state).report.session_epoch;
            tracing::trace!(target: "rds_desktop::input_timing", viewer_epoch=epoch, input_seq=event.seq,event_class,event_created_ms=event.event_ts_ms,input_sent_ms=sent_ms,queue_ms=sent_ms.saturating_sub(event.event_ts_ms),"native input dispatched");
        }
    }
    pub fn video_repair_requested(&self) {
        lock(&self.state).report.video_repair_requests += 1;
    }
    pub fn managed_events_separated(&self) {
        lock(&self.state).report.managed_events_separated = true;
    }
    pub fn input_ack(&self, seq: u64) {
        let mut state = lock(&self.state);
        state.report.input_acks += 1;
        let ack_ms = self.started.elapsed().as_millis() as u64;
        let latency = state.input_latency.ack(seq, ack_ms);
        state.report.last_input_ack_ms = latency;
        let epoch = state.report.session_epoch;
        let slow = latency.is_some_and(|ms| ms >= 250.);
        if slow {
            state.report.slow_input_acks += 1;
        }
        let log_slow = slow
            && state
                .input_latency
                .last_slow_log_ms
                .is_none_or(|last| ack_ms.saturating_sub(last) >= 1000);
        if log_slow {
            state.input_latency.last_slow_log_ms = Some(ack_ms);
        }
        if latency.is_some() {
            state.report.input_acks_matched += 1;
        } else {
            state.report.input_acks_unmatched += 1;
        }
        drop(state);
        if log_slow {
            tracing::warn!(target: "rds_desktop::input_timing", viewer_epoch=epoch, input_seq=seq, event_to_ack_ms=?latency, "native input acknowledgement delayed");
        }
        tracing::trace!(target: "rds_desktop::input_timing", viewer_epoch=epoch, input_seq=seq, input_ack_ms=ack_ms,
            event_to_ack_ms=?latency, "native input acknowledgement observed");
    }
    pub fn clipboard_ready(&self, bytes: u32) {
        let mut state = lock(&self.state);
        state.report.clipboard_transfers += 1;
        state.report.last_clipboard_bytes = bytes;
    }
    /// Sender stage durations use only that sender's monotonic clock. They
    /// are separate from network transit and local receive-to-submit timing.
    pub fn media_timing(&self, header: &rds_core::FrameHeader) {
        let mut state = lock(&self.state);
        state.last_encoded_ms = Some(self.started.elapsed().as_millis() as u64);
        sample(
            &mut state.encoding,
            header
                .encode_done_ts_ms
                .saturating_sub(header.capture_ts_ms) as f64,
        );
        sample(
            &mut state.sending,
            header.send_ts_ms.saturating_sub(header.encode_done_ts_ms) as f64,
        );
    }
    pub fn close(&self) {
        let mut state = lock(&self.state);
        state.close = true;
        self.wake(&mut state);
    }
    pub fn report(&self) -> ViewerReport {
        let state = lock(&self.state);
        let mut report = state.report.clone();
        (
            report.receive_to_submit_p50_ms,
            report.receive_to_submit_p95_ms,
        ) = quantiles(&state.delays);
        (report.capture_encode_p50_ms, report.capture_encode_p95_ms) = quantiles(&state.encoding);
        (report.encode_to_send_p50_ms, report.encode_to_send_p95_ms) = quantiles(&state.sending);
        (report.input_ack_p50_ms, report.input_ack_p95_ms) =
            quantiles(&state.input_latency.acknowledged);
        report.input_queue_p95_ms = quantiles(&state.input_latency.queued).1;
        report.input_ack_tracking_evicted = state.input_latency.evicted;
        report.pending_input_acks = state.input_latency.pending.len();
        report.oldest_input_ack_age_ms = state.input_latency.pending.front().map(|(_, created)| {
            (self.started.elapsed().as_millis() as u64).saturating_sub(*created)
        });
        report.input_pointer_coalesced = self.input_state.pointer_coalesced.load(Ordering::Relaxed);
        report.input_events_dropped = self.input_state.input_dropped.load(Ordering::Relaxed);
        report.input_queue_max_depth = self.input_state.max_depth.load(Ordering::Relaxed);
        report.input_queue_depth = self
            .input_state
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        report.visual_probe = state.visual_probe.as_ref().map(|probe| probe.report());
        report
    }
}

pub struct Viewer {
    event_loop: EventLoop<()>,
    app: App,
}
impl Viewer {
    /// Must be constructed and run on the OS main thread. No identity or socket
    /// is owned by the viewer; the caller owns its asynchronous session task.
    pub fn new(display: u32) -> Result<(Self, ViewerHandle, InputReceiver), DesktopError> {
        let event_loop =
            super::platform::event_loop().map_err(|e| DesktopError::Capture(e.to_string()))?;
        let input_state = Arc::new(InputState {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
            pointer_coalesced: AtomicU64::new(0),
            input_dropped: AtomicU64::new(0),
            max_depth: AtomicU64::new(0),
        });
        let (input, receiver) = (
            InputSender(input_state.clone()),
            InputReceiver(input_state.clone()),
        );
        let handle = ViewerHandle {
            state: Arc::new(Mutex::new(State {
                pending: None,
                wake_pending: false,
                status: "Connecting".into(),
                extent: (0, 0),
                report: ViewerReport::default(),
                delays: Vec::new(),
                encoding: Vec::new(),
                sending: Vec::new(),
                input_latency: InputLatency::default(),
                close: false,
                interrupted: None,
                network_stage: "starting".into(),
                render_stage: "starting".into(),
                last_ui_ms: 0,
                last_encoded_ms: None,
                occluded: false,
                visual_probe: None,
                session_span: tracing::Span::none(),
            })),
            input_state,
            proxy: event_loop.create_proxy(),
            started: Instant::now(),
        };
        let app = App {
            window: None,
            gpu: None,
            handle: handle.clone(),
            input,
            display,
            seq: 0,
            modifiers: Default::default(),
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            pointer: false,
            pointer_point: None,
            error: None,
        };
        Ok((Self { event_loop, app }, handle, receiver))
    }
    pub fn run(mut self) -> Result<(), DesktopError> {
        let _activity = super::platform::remote_activity();
        self.event_loop
            .run_app(&mut self.app)
            .map_err(|e| DesktopError::Capture(e.to_string()))?;
        match self.app.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    handle: ViewerHandle,
    input: InputSender,
    display: u32,
    seq: u64,
    modifiers: winit::event::Modifiers,
    keys: BTreeSet<u32>,
    buttons: BTreeSet<i32>,
    pointer: bool,
    pointer_point: Option<(f64, f64)>,
    error: Option<DesktopError>,
}
impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: DesktopError) {
        self.error = Some(error);
        let _ = self.input.send(ViewerInput::Close);
        event_loop.exit();
    }
    fn sync_modifiers(&mut self, event_loop: &ActiveEventLoop) {
        for (code, pressed) in modifier_changes(&self.keys, self.modifiers) {
            if pressed {
                self.keys.insert(code);
            } else {
                self.keys.remove(&code);
            }
            self.input(
                event_loop,
                if pressed {
                    InputKind::KeyDown { code }
                } else {
                    InputKind::KeyUp { code }
                },
            );
        }
    }
    fn input(&mut self, event_loop: &ActiveEventLoop, kind: InputKind) {
        let Some(next) = self.seq.checked_add(1) else {
            self.fail(
                event_loop,
                DesktopError::Input("input sequence exhausted".into()),
            );
            return;
        };
        let message = DesktopControl::Input(InputEvent {
            seq: self.seq,
            event_ts_ms: self.handle.started.elapsed().as_millis() as u64,
            display_id: self.display,
            kind,
        });
        if matches!(
            &message,
            DesktopControl::Input(InputEvent {
                kind: InputKind::PointerButton {
                    button: 0x110,
                    pressed: true
                },
                ..
            })
        ) && let Some(point) = self.pointer_point
            && let Some(probe) = &mut lock(&self.handle.state).visual_probe
        {
            probe.click(point, self.seq, Instant::now());
        }
        self.seq = next;
        if self.input.send(ViewerInput::Control(message)).is_err() {
            // Closing the remote session releases its held keys/buttons. Never
            // silently discard a key-up and leave a modifier stuck remotely.
            self.fail(
                event_loop,
                DesktopError::Input("input channel unavailable".into()),
            );
        }
    }
    fn release(&mut self, event_loop: &ActiveEventLoop) {
        for code in std::mem::take(&mut self.keys) {
            self.input(event_loop, InputKind::KeyUp { code });
        }
        for button in std::mem::take(&mut self.buttons) {
            self.input(
                event_loop,
                InputKind::PointerButton {
                    button,
                    pressed: false,
                },
            );
        }
    }
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        lock(&self.handle.state).report.redraws += 1;
        let pending = lock(&self.handle.state).pending.take();
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        lock(&self.handle.state).render_stage = "acquiring surface".into();
        let result = gpu.draw(pending.as_ref().map(|frame| &frame.raw));
        lock(&self.handle.state).report.gpu_uploads = gpu.uploads();
        match result {
            Err(error) => self.fail(event_loop, error),
            Ok(DrawOutcome::Presented) => {
                lock(&self.handle.state).render_stage = "presented".into();
                if let Some(frame) = pending {
                    let mut state = lock(&self.handle.state);
                    let parent = state.session_span.clone();
                    tracing::trace!(target:"rds_desktop::frame_timing", parent:&parent, frame_seq=frame.frame_seq,
                        viewer_epoch=state.report.session_epoch, receive_to_submit_ms=frame.received.elapsed().as_secs_f64()*1000.,
                        "native frame submitted");
                    if let Some(probe) = &mut state.visual_probe {
                        parent.in_scope(|| {
                            probe.presented_with_seq(&frame.raw, Instant::now(), frame.frame_seq)
                        });
                    }
                    state.report.frames_submitted += 1;
                    state.report.last_submission_ms =
                        Some(self.handle.started.elapsed().as_millis() as u64);
                    state.render_stage = "presented".into();
                    state
                        .report
                        .first_frame_ms
                        .get_or_insert(self.handle.started.elapsed().as_millis() as u64);
                    sample(
                        &mut state.delays,
                        frame.received.elapsed().as_secs_f64() * 1000.,
                    );
                }
            }
            Ok(outcome) => {
                if matches!(outcome, DrawOutcome::Occluded)
                    && let Some(probe) = &mut lock(&self.handle.state).visual_probe
                {
                    probe.unavailable();
                }
                lock(&self.handle.state).render_stage = outcome.stage().into();
                lock(&self.handle.state).report.surface_skips += 1;
                if let Some(frame) = pending {
                    let mut state = lock(&self.handle.state);
                    if state.pending.is_none() {
                        state.pending = Some(frame);
                    }
                }
            }
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let icon = Icon::from_rgba(
            include_bytes!("../../assets/app-icon-256.rgba").to_vec(),
            256,
            256,
        );
        let result = icon
            .map_err(|e| DesktopError::Capture(format!("application icon: {e}")))
            .and_then(|icon| {
                event_loop
                    .create_window(
                        super::platform::window_attributes()
                            .with_title("RDS — Connecting")
                            .with_window_level(WindowLevel::Normal)
                            .with_window_icon(Some(icon))
                            .with_inner_size(LogicalSize::new(1280., 720.)),
                    )
                    .map_err(|e| DesktopError::Capture(e.to_string()))
            })
            .and_then(|window| {
                let window = Arc::new(window);
                Gpu::new(window.clone(), event_loop.owned_display_handle()).map(|gpu| (window, gpu))
            });
        match result {
            Ok((window, gpu)) => {
                // Explicit viewer launches activate once like ordinary apps.
                // Reconnect/redraw never raises the window over other apps.
                super::platform::activate_application();
                window.focus_window();
                window.request_redraw();
                self.window = Some(window);
                self.gpu = Some(gpu);
            }
            Err(error) => self.fail(event_loop, error),
        }
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        let mut state = lock(&self.handle.state);
        state.wake_pending = false;
        let close = state.close;
        state.last_ui_ms = self.handle.started.elapsed().as_millis() as u64;
        let status = state.status.clone();
        drop(state);
        if let Some(window) = &self.window {
            window.set_title(&format!("RDS — {status}"));
            window.request_redraw();
        }
        if close {
            self.release(event_loop);
            event_loop.exit();
        }
    }
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        lock(&self.handle.state).last_ui_ms = self.handle.started.elapsed().as_millis() as u64;
        match event {
            WindowEvent::CloseRequested => {
                self.release(event_loop);
                let _ = self.input.send(ViewerInput::Close);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::Focused(false) => self.release(event_loop),
            WindowEvent::Occluded(hidden) => {
                let mut state = lock(&self.handle.state);
                state.occluded = hidden;
                if hidden && let Some(probe) = &mut state.visual_probe {
                    probe.unavailable();
                }
                drop(state);
                if !hidden && let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(true) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                self.sync_modifiers(event_loop);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key
                    && let Some(code) = evdev(key)
                {
                    if event.repeat {
                        // Repeat only a key actually forwarded and still held.
                        // Never re-run clipboard/local shortcut side effects.
                        if event.state == ElementState::Pressed
                            && self.keys.contains(&code)
                            && !matches!(
                                code,
                                29 | 42 | 54 | 56 | 97 | 100 | 125 | 126 | 58 | 69 | 70
                            )
                        {
                            self.input(event_loop, InputKind::KeyDown { code });
                        }
                        return;
                    }
                    if !matches!(code, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126) {
                        self.sync_modifiers(event_loop);
                    }
                    if event.state == ElementState::Pressed {
                        let command_paste = cfg!(target_os = "macos")
                            && code == 47
                            && (self.keys.contains(&125) || self.keys.contains(&126));
                        if code == 47
                            && (command_paste || self.keys.contains(&29) || self.keys.contains(&97))
                        {
                            match super::platform::paste_text() {
                                Ok(Some(text)) => {
                                    let id = rand::random();
                                    let total = text.len() as u32;
                                    tracing::info!(
                                        bytes = total,
                                        command_paste,
                                        "explicit local clipboard text queued"
                                    );
                                    let chunks =
                                        text.as_bytes().chunks(crate::clipboard::SEND_CHUNK_BYTES);
                                    for (index, chunk) in chunks.enumerate() {
                                        if self
                                            .input
                                            .send(ViewerInput::Control(
                                                DesktopControl::ClipboardChunk {
                                                    id,
                                                    offset: (index
                                                        * crate::clipboard::SEND_CHUNK_BYTES)
                                                        as u32,
                                                    total,
                                                    data: chunk.to_vec(),
                                                },
                                            ))
                                            .is_err()
                                        {
                                            self.fail(
                                                event_loop,
                                                DesktopError::Input(
                                                    "clipboard input queue full".into(),
                                                ),
                                            );
                                            return;
                                        }
                                    }
                                    if total == 0
                                        && self
                                            .input
                                            .send(ViewerInput::Control(
                                                DesktopControl::ClipboardChunk {
                                                    id,
                                                    offset: 0,
                                                    total,
                                                    data: vec![],
                                                },
                                            ))
                                            .is_err()
                                    {
                                        self.fail(
                                            event_loop,
                                            DesktopError::Input(
                                                "clipboard input queue full".into(),
                                            ),
                                        );
                                        return;
                                    }
                                }
                                Ok(None) if command_paste => {
                                    tracing::info!("explicit local clipboard paste has no text");
                                    return;
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    self.handle
                                        .status(format!("Clipboard unavailable: {error}"));
                                    return;
                                }
                            }
                        }
                        if command_paste {
                            for kind in super::input::command_paste_chord(&self.keys) {
                                self.input(event_loop, kind);
                            }
                            // The physical V is consumed locally: its release
                            // and OS repeats must not replay paste effects.
                            return;
                        }
                        self.keys.insert(code);
                        self.input(event_loop, InputKind::KeyDown { code });
                    } else if self.keys.remove(&code) {
                        self.input(event_loop, InputKind::KeyUp { code });
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let extent = lock(&self.handle.state).extent;
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    let point = Viewport::new(size.width, size.height, extent.0, extent.1)
                        .pointer(position.x, position.y, extent.0, extent.1);
                    self.pointer = point.is_some();
                    self.pointer_point = point;
                    if let Some((x, y)) = point {
                        self.input(event_loop, InputKind::PointerMove { x, y });
                    }
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                self.sync_modifiers(event_loop);
                let button = match button {
                    MouseButton::Left => 0x110,
                    MouseButton::Right => 0x111,
                    MouseButton::Middle => 0x112,
                    _ => return,
                };
                if state == ElementState::Pressed && self.pointer {
                    self.buttons.insert(button);
                    self.input(
                        event_loop,
                        InputKind::PointerButton {
                            button,
                            pressed: true,
                        },
                    );
                } else if state == ElementState::Released && self.buttons.remove(&button) {
                    self.input(
                        event_loop,
                        InputKind::PointerButton {
                            button,
                            pressed: false,
                        },
                    );
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.pointer => {
                self.sync_modifiers(event_loop);
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                    MouseScrollDelta::PixelDelta(p) => (p.x / 40., p.y / 40.),
                };
                self.input(event_loop, InputKind::Scroll { dx, dy });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(seq: u64, kind: InputKind) -> ViewerInput {
        ViewerInput::Control(DesktopControl::Input(InputEvent {
            seq,
            event_ts_ms: 0,
            display_id: 0,
            kind,
        }))
    }
    #[test]
    fn input_latency_matches_sequences_and_keeps_bounded_local_clock_history() {
        let mut latency = InputLatency::default();
        latency.sent(7, 100, 120);
        latency.sent(8, 110, 125);
        assert_eq!(latency.ack(8, 310), Some(200.));
        assert_eq!(latency.ack(8, 410), None); // duplicate is not another sample
        assert_eq!(latency.ack(999, 510), None); // unrelated ACK cannot correlate
        assert_eq!(latency.acknowledged, vec![200.]);
        assert_eq!(latency.queued, vec![20., 15.]);
        assert_eq!(latency.pending.front(), Some(&(7, 100)));
        for seq in 100..1500 {
            latency.sent(seq, seq, seq + 3);
        }
        assert_eq!(latency.pending.len(), INPUT_QUEUE_CAPACITY);
        assert_eq!(latency.queued.len(), 1024);
        assert_eq!(latency.pending.front(), Some(&(476, 476)));
        assert_eq!(latency.evicted, 377);
    }

    #[tokio::test]
    async fn pointer_collapse_keeps_click_and_release_order_and_close_wakes_receiver() {
        let state = Arc::new(InputState {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
            pointer_coalesced: AtomicU64::new(0),
            input_dropped: AtomicU64::new(0),
            max_depth: AtomicU64::new(0),
        });
        let sender = InputSender(state.clone());
        let mut receiver = InputReceiver(state);
        sender
            .send(event(0, InputKind::PointerMove { x: 1., y: 1. }))
            .unwrap();
        sender
            .send(event(1, InputKind::PointerMove { x: 2., y: 2. }))
            .unwrap();
        sender
            .send(event(
                2,
                InputKind::PointerButton {
                    button: 0x110,
                    pressed: true,
                },
            ))
            .unwrap();
        sender
            .send(event(3, InputKind::PointerMove { x: 3., y: 3. }))
            .unwrap();
        sender
            .send(event(
                4,
                InputKind::PointerButton {
                    button: 0x110,
                    pressed: false,
                },
            ))
            .unwrap();
        for expected in [1, 2, 3, 4] {
            let Some(ViewerInput::Control(DesktopControl::Input(event))) = receiver.recv().await
            else {
                panic!("missing input");
            };
            assert_eq!(event.seq, expected);
        }
        drop(sender);
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
                .await
                .unwrap(),
            Some(ViewerInput::Close)
        ));
    }

    #[tokio::test]
    async fn rapid_semantic_burst_preserves_every_button_event() {
        let state = Arc::new(InputState {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
            pointer_coalesced: AtomicU64::new(0),
            input_dropped: AtomicU64::new(0),
            max_depth: AtomicU64::new(0),
        });
        let sender = InputSender(state.clone());
        let receiver = InputReceiver(state.clone());
        for seq in 0..400 {
            sender
                .send(event(
                    seq * 2,
                    InputKind::PointerButton {
                        button: 0x110,
                        pressed: true,
                    },
                ))
                .unwrap();
            sender
                .send(event(
                    seq * 2 + 1,
                    InputKind::PointerButton {
                        button: 0x110,
                        pressed: false,
                    },
                ))
                .unwrap();
        }
        let mut seen = Vec::with_capacity(800);
        while let Some(ViewerInput::Control(DesktopControl::Input(input))) = receiver.try_recv() {
            seen.push(input.seq);
        }
        assert_eq!(seen.len(), 800);
        assert_eq!(seen, (0..800).collect::<Vec<_>>());
        assert_eq!(state.input_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(state.pointer_coalesced.load(Ordering::Relaxed), 0);
        assert_eq!(state.max_depth.load(Ordering::Relaxed), 800);
    }

    fn test_input_state() -> Arc<InputState> {
        Arc::new(InputState {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
            pointer_coalesced: AtomicU64::new(0),
            input_dropped: AtomicU64::new(0),
            max_depth: AtomicU64::new(0),
        })
    }

    #[test]
    fn semantic_burst_acknowledgements_remain_correlated_when_replies_wait() {
        let mut latency = InputLatency::default();
        for seq in 0..800 {
            latency.sent(seq, seq, 800);
        }
        assert_eq!(latency.pending.len(), 800);
        assert_eq!(latency.evicted, 0);
        // Replies can be observed after dispatch has already sent the burst.
        for seq in (0..800).rev() {
            assert_eq!(latency.ack(seq, 1000), Some((1000 - seq) as f64));
        }
        assert!(latency.pending.is_empty());
        assert_eq!(latency.acknowledged.len(), 800);
        assert_eq!(latency.ack(799, 1001), None);
    }

    #[test]
    fn full_queue_never_removes_a_position_before_a_click() {
        for incoming in [
            InputKind::PointerMove { x: 999., y: 999. },
            InputKind::KeyDown { code: 30 },
            InputKind::PointerButton {
                button: 0x110,
                pressed: false,
            },
        ] {
            let state = test_input_state();
            let sender = InputSender(state.clone());
            let receiver = InputReceiver(state.clone());
            for seq in 0..INPUT_QUEUE_CAPACITY as u64 {
                let kind = if seq == INPUT_QUEUE_CAPACITY as u64 - 1 {
                    InputKind::KeyUp { code: 30 }
                } else {
                    match seq % 3 {
                        0 => InputKind::PointerMove {
                            x: seq as f64,
                            y: 1.,
                        },
                        1 => InputKind::PointerButton {
                            button: 0x110,
                            pressed: true,
                        },
                        _ => InputKind::PointerButton {
                            button: 0x110,
                            pressed: false,
                        },
                    }
                };
                sender.send(event(seq, kind)).unwrap();
            }
            // End on a semantic barrier; incoming motion must not remove
            // an earlier click's position to make room.
            assert!(
                sender
                    .send(event(INPUT_QUEUE_CAPACITY as u64, incoming))
                    .is_err()
            );
            for seq in 0..INPUT_QUEUE_CAPACITY as u64 {
                let Some(ViewerInput::Control(DesktopControl::Input(input))) = receiver.try_recv()
                else {
                    panic!("accepted action lost");
                };
                assert_eq!(input.seq, seq);
                if seq % 3 == 0 && seq < INPUT_QUEUE_CAPACITY as u64 - 1 {
                    assert!(
                        matches!(input.kind, InputKind::PointerMove { x, .. } if x == seq as f64)
                    );
                }
            }
            assert!(receiver.try_recv().is_none());
            assert_eq!(state.pointer_coalesced.load(Ordering::Relaxed), 0);
            assert_eq!(state.input_dropped.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn full_queue_can_still_replace_same_display_tail_motion() {
        let state = test_input_state();
        let sender = InputSender(state.clone());
        for seq in 0..INPUT_QUEUE_CAPACITY as u64 - 1 {
            sender
                .send(event(seq, InputKind::KeyDown { code: 30 }))
                .unwrap();
        }
        sender
            .send(event(1023, InputKind::PointerMove { x: 1., y: 1. }))
            .unwrap();
        sender
            .send(event(1024, InputKind::PointerMove { x: 2., y: 2. }))
            .unwrap();
        assert_eq!(state.queue.lock().unwrap().len(), INPUT_QUEUE_CAPACITY);
        assert_eq!(state.pointer_coalesced.load(Ordering::Relaxed), 1);
        assert_eq!(state.input_dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pointer_coalescing_does_not_cross_displays() {
        let state = test_input_state();
        let sender = InputSender(state.clone());
        let receiver = InputReceiver(state.clone());
        sender
            .send(event(0, InputKind::PointerMove { x: 1., y: 1. }))
            .unwrap();
        let ViewerInput::Control(DesktopControl::Input(mut other)) =
            event(1, InputKind::PointerMove { x: 2., y: 2. })
        else {
            unreachable!()
        };
        other.display_id = 1;
        sender
            .send(ViewerInput::Control(DesktopControl::Input(other)))
            .unwrap();
        assert!(receiver.try_recv().is_some());
        assert!(receiver.try_recv().is_some());
        assert_eq!(state.pointer_coalesced.load(Ordering::Relaxed), 0);
    }
}
