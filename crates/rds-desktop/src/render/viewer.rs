use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex},
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
}
struct InputSender(Arc<InputState>);
/// One bounded input consumer. Consecutive pointer positions collapse, while
/// keys/buttons retain their order relative to the final pointer before them.
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
        if matches!(
            &message,
            ViewerInput::Control(DesktopControl::Input(InputEvent {
                kind: InputKind::PointerMove { .. },
                ..
            }))
        ) && matches!(
            queue.back(),
            Some(ViewerInput::Control(DesktopControl::Input(InputEvent {
                kind: InputKind::PointerMove { .. },
                ..
            })))
        ) {
            queue.pop_back();
        }
        if queue.len() >= 128 {
            return Err(());
        }
        queue.push_back(message);
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
    pub input_acks: u64,
    pub input_ack_p50_ms: Option<f64>,
    pub input_ack_p95_ms: Option<f64>,
    pub input_queue_p95_ms: Option<f64>,
    pub pending_input_acks: usize,
    pub oldest_input_ack_age_ms: Option<u64>,
    pub clipboard_transfers: u64,
    pub last_clipboard_bytes: u32,
    pub reconnects: u64,
    pub last_recovery_ms: Option<u64>,
    pub last_frame_ms: Option<u64>,
    pub video_width: u32,
    pub video_height: u32,
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
    pub submission_age_ms: Option<u64>,
    pub pending_frame_bytes: usize,
    pub occluded: bool,
    pub report: ViewerReport,
}

struct Pending {
    raw: RawFrame,
    received: Instant,
}
/// Local event-to-ack measurements never compare clocks on different hosts.
/// Coalesced pointer events are tracked only after dequeue; the event's own
/// local creation time still includes time spent waiting in the UI queue.
#[derive(Default)]
struct InputLatency {
    pending: VecDeque<(u64, u64)>,
    queued: Vec<f64>,
    acknowledged: Vec<f64>,
}
impl InputLatency {
    fn sent(&mut self, seq: u64, created_ms: u64, now_ms: u64) {
        if self.pending.len() == 128 {
            self.pending.pop_front();
        }
        self.pending.push_back((seq, created_ms));
        sample(&mut self.queued, now_ms.saturating_sub(created_ms) as f64);
    }
    fn ack(&mut self, seq: u64, now_ms: u64) {
        if let Some(index) = self.pending.iter().position(|(pending, _)| *pending == seq)
            && let Some((_, created_ms)) = self.pending.remove(index)
        {
            sample(
                &mut self.acknowledged,
                now_ms.saturating_sub(created_ms) as f64,
            );
        }
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
        let mut state = lock(&self.state);
        state.report.frames_received += 1;
        state.report.video_width = raw.width;
        state.report.video_height = raw.height;
        state.report.last_frame_ms = Some(self.started.elapsed().as_millis() as u64);
        if let Some(interrupted) = state.interrupted.take() {
            state.report.last_recovery_ms = Some(interrupted.elapsed().as_millis() as u64);
        }
        state.status = "Connected".into();
        if state.pending.replace(Pending { raw, received }).is_some() {
            state.report.frames_replaced += 1;
        }
        self.wake(&mut state);
    }
    pub fn status(&self, text: impl Into<String>) {
        let mut state = lock(&self.state);
        state.status = text.into();
        if state.status == "Reconnecting" {
            state.input_latency.pending.clear();
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
    pub fn control_rtt(&self, ms: u64) {
        lock(&self.state).report.control_rtt_ms = Some(ms);
    }
    pub fn input_sent(&self, control: &DesktopControl) {
        if let DesktopControl::Input(event) = control {
            lock(&self.state).input_latency.sent(
                event.seq,
                event.event_ts_ms,
                self.started.elapsed().as_millis() as u64,
            );
        }
    }
    pub fn input_ack(&self, seq: u64) {
        let mut state = lock(&self.state);
        state.report.input_acks += 1;
        state
            .input_latency
            .ack(seq, self.started.elapsed().as_millis() as u64);
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
        report.pending_input_acks = state.input_latency.pending.len();
        report.oldest_input_ack_age_ms = state.input_latency.pending.front().map(|(_, created)| {
            (self.started.elapsed().as_millis() as u64).saturating_sub(*created)
        });
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
        let queue = Arc::new(InputState {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
        });
        let (input, receiver) = (InputSender(queue.clone()), InputReceiver(queue));
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
            })),
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
            error: None,
        };
        Ok((Self { event_loop, app }, handle, receiver))
    }
    pub fn run(mut self) -> Result<(), DesktopError> {
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
                if let Some(frame) = pending {
                    let mut state = lock(&self.handle.state);
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
                lock(&self.handle.state).occluded = hidden;
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
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                if let PhysicalKey::Code(key) = event.physical_key
                    && let Some(code) = evdev(key)
                {
                    if !matches!(code, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126) {
                        self.sync_modifiers(event_loop);
                    }
                    if event.state == ElementState::Pressed {
                        if code == 47 && (self.keys.contains(&29) || self.keys.contains(&97)) {
                            match super::platform::paste_text() {
                                Ok(Some(text)) => {
                                    let id = rand::random();
                                    let total = text.len() as u32;
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
                                Ok(None) => {}
                                Err(error) => {
                                    self.handle
                                        .status(format!("Clipboard unavailable: {error}"));
                                    return;
                                }
                            }
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
        latency.ack(8, 310);
        latency.ack(8, 410); // duplicate must not produce another sample
        latency.ack(999, 510); // unrelated ACK must not corrupt correlation
        assert_eq!(latency.acknowledged, vec![200.]);
        assert_eq!(latency.queued, vec![20., 15.]);
        assert_eq!(latency.pending.front(), Some(&(7, 100)));
        for seq in 100..1500 {
            latency.sent(seq, seq, seq + 3);
        }
        assert_eq!(latency.pending.len(), 128);
        assert_eq!(latency.queued.len(), 1024);
        assert_eq!(latency.pending.front(), Some(&(1372, 1372)));
    }

    #[tokio::test]
    async fn pointer_collapse_keeps_click_and_release_order_and_close_wakes_receiver() {
        let state = Arc::new(InputState {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
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
}
