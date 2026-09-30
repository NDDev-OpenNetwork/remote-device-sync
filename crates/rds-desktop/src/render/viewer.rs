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
    window::{Window, WindowId},
};

use super::{
    gpu::Gpu,
    input::{Viewport, evdev},
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
    pub frames_replaced: u64,
    pub first_frame_ms: Option<u64>,
    pub receive_to_submit_p50_ms: Option<f64>,
    pub receive_to_submit_p95_ms: Option<f64>,
    pub control_rtt_ms: Option<u64>,
    pub input_acks: u64,
    pub reconnects: u64,
    pub last_recovery_ms: Option<u64>,
    pub last_frame_ms: Option<u64>,
    pub video_width: u32,
    pub video_height: u32,
}

struct Pending {
    raw: RawFrame,
    received: Instant,
}
struct State {
    pending: Option<Pending>,
    wake_pending: bool,
    status: String,
    extent: (u32, u32),
    report: ViewerReport,
    delays: Vec<f64>,
    close: bool,
    interrupted: Option<Instant>,
}
fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
pub struct ViewerHandle {
    state: Arc<Mutex<State>>,
    proxy: EventLoopProxy<()>,
    started: Instant,
}
impl ViewerHandle {
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
    pub fn input_ack(&self) {
        lock(&self.state).report.input_acks += 1;
    }
    pub fn close(&self) {
        let mut state = lock(&self.state);
        state.close = true;
        self.wake(&mut state);
    }
    pub fn report(&self) -> ViewerReport {
        let state = lock(&self.state);
        let mut report = state.report.clone();
        if !state.delays.is_empty() {
            let mut samples = state.delays.clone();
            samples.sort_by(f64::total_cmp);
            report.receive_to_submit_p50_ms = Some(samples[(samples.len() - 1) * 50 / 100]);
            report.receive_to_submit_p95_ms = Some(samples[(samples.len() - 1) * 95 / 100]);
        }
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
        let event_loop = EventLoop::with_user_event()
            .build()
            .map_err(|e| DesktopError::Capture(e.to_string()))?;
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
                close: false,
                interrupted: None,
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
        let pending = lock(&self.handle.state).pending.take();
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let result = match &pending {
            Some(frame) => gpu.upload(&frame.raw).and_then(|()| gpu.draw()),
            None => gpu.draw(),
        };
        match result {
            Err(error) => self.fail(event_loop, error),
            Ok(true) => {
                if let Some(frame) = pending {
                    let mut state = lock(&self.handle.state);
                    state.report.frames_submitted += 1;
                    state
                        .report
                        .first_frame_ms
                        .get_or_insert(self.handle.started.elapsed().as_millis() as u64);
                    if state.delays.len() == 1024 {
                        state.delays.remove(0);
                    }
                    state
                        .delays
                        .push(frame.received.elapsed().as_secs_f64() * 1000.);
                }
            }
            Ok(false) => {
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
        let result = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title("RDS — Connecting")
                    .with_inner_size(LogicalSize::new(1280., 720.)),
            )
            .map_err(|e| DesktopError::Capture(e.to_string()))
            .and_then(|window| {
                let window = Arc::new(window);
                Gpu::new(window.clone(), event_loop.owned_display_handle()).map(|gpu| (window, gpu))
            });
        match result {
            Ok((window, gpu)) => {
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
        if let Some(window) = &self.window {
            window.set_title(&format!("RDS — {}", state.status));
            window.request_redraw();
        }
        drop(state);
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
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                if let PhysicalKey::Code(key) = event.physical_key
                    && let Some(code) = evdev(key)
                {
                    if event.state == ElementState::Pressed {
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
