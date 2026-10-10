//! The native workspace owns tab presentation; the caller owns network tasks.
use super::*;
use crate::render::workspace::chrome::{Action, Chrome};
use crate::render::workspace::{DeviceView, Tab, TabId, TabSpec, WorkspaceModel};
use std::collections::BTreeMap;

pub enum WorkspaceEvent {
    Start {
        tab: Tab,
        view: ViewerHandle,
        input: InputReceiver,
    },
    Stop(TabId),
    Inspect {
        target: String,
        grant_file: String,
    },
    Save {
        tabs: Vec<TabSpec>,
    },
}

pub enum WorkspaceUpdate {
    Device(DeviceView),
    Open(TabSpec),
    Message(String),
    CloseWindow,
}

#[derive(Clone)]
pub struct WorkspaceHandle {
    updates: std::sync::mpsc::SyncSender<WorkspaceUpdate>,
    proxy: EventLoopProxy<()>,
}
impl WorkspaceHandle {
    pub fn update(&self, update: WorkspaceUpdate) -> Result<(), DesktopError> {
        self.updates
            .try_send(update)
            .map_err(|_| DesktopError::Input("workspace update queue unavailable".into()))?;
        self.proxy
            .send_event(())
            .map_err(|_| DesktopError::Input("workspace closed".into()))
    }
}

pub(super) struct Deck {
    pub model: WorkspaceModel,
    inactive: BTreeMap<TabId, SessionView>,
    pub active: Option<TabId>,
    pub devices: Vec<DeviceView>,
    pub chrome: Option<Chrome>,
    updates: std::sync::mpsc::Receiver<WorkspaceUpdate>,
    events: tokio::sync::mpsc::Sender<WorkspaceEvent>,
    pub message: String,
}

impl Viewer {
    pub fn workspace(
        initial: Vec<TabSpec>,
        devices: Vec<DeviceView>,
    ) -> Result<
        (
            Self,
            WorkspaceHandle,
            tokio::sync::mpsc::Receiver<WorkspaceEvent>,
        ),
        DesktopError,
    > {
        // Validate the complete launch before any session worker can start.
        if devices.len() > 32 {
            return Err(DesktopError::Input(
                "workspace device limit exceeded".into(),
            ));
        }
        for device in &devices {
            device
                .validate()
                .map_err(|e| DesktopError::Input(e.to_string()))?;
        }
        let mut model = WorkspaceModel::default();
        for spec in &initial {
            model
                .open(spec.clone())
                .map_err(|e| DesktopError::Input(e.to_string()))?;
        }
        let (mut viewer, _view, _input) = Self::new(0)?;
        let (updates, receive_updates) = std::sync::mpsc::sync_channel(64);
        let (events, receive_events) = tokio::sync::mpsc::channel(32);
        viewer.app.workspace = Some(Deck {
            model: WorkspaceModel::default(),
            inactive: BTreeMap::new(),
            active: None,
            devices,
            chrome: None,
            updates: receive_updates,
            events,
            message: String::new(),
        });
        let handle = WorkspaceHandle {
            updates,
            proxy: viewer.event_loop.create_proxy(),
        };
        for spec in initial {
            handle.update(WorkspaceUpdate::Open(spec))?;
        }
        Ok((viewer, handle, receive_events))
    }
}

impl App {
    pub(super) fn workspace_updates(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut deck) = self.workspace.take() else {
            return;
        };
        let mut closed = vec![];
        for (id, session) in &deck.inactive {
            let mut state = lock(&session.handle.state);
            state.wake_pending = false;
            state.last_ui_ms = session.handle.started.elapsed().as_millis() as u64;
            if state.close {
                closed.push(*id);
            }
        }
        for id in closed {
            self.workspace_close(&mut deck, id, event_loop);
        }
        for _ in 0..64 {
            let Ok(update) = deck.updates.try_recv() else {
                break;
            };
            match update {
                WorkspaceUpdate::CloseWindow => {
                    self.release(event_loop);
                    event_loop.exit();
                    break;
                }
                WorkspaceUpdate::Open(spec) => self.workspace_open(&mut deck, spec, event_loop),
                WorkspaceUpdate::Message(message) => {
                    deck.message = message.chars().take(512).collect()
                }
                WorkspaceUpdate::Device(device) => {
                    if let Err(error) = device.validate() {
                        deck.message = error.to_string();
                        continue;
                    }
                    if let Some(existing) = deck.devices.iter_mut().find(|d| d.key == device.key) {
                        *existing = device;
                    } else if deck.devices.len() < 32 {
                        deck.devices.push(device);
                    }
                    deck.message = "Select the displays to open".into();
                }
            }
        }
        if let Some(chrome) = &mut deck.chrome {
            chrome.message = deck.message.clone();
        }
        self.workspace = Some(deck);
    }

    pub(super) fn workspace_close_active(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut deck) = self.workspace.take() else {
            return;
        };
        if let Some(id) = deck.active {
            self.workspace_close(&mut deck, id, event_loop);
        }
        self.workspace = Some(deck);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// UI gestures never also reach a remote desktop. Release any held input
    /// before entering local widgets, switching or turning off interaction.
    pub(super) fn workspace_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &WindowEvent,
    ) -> bool {
        let Some(mut deck) = self.workspace.take() else {
            return false;
        };
        if let WindowEvent::ModifiersChanged(modifiers) = event {
            self.session.modifiers = *modifiers;
        }
        let mut consumed = false;
        if let (Some(chrome), Some(window)) = (&mut deck.chrome, &self.window) {
            consumed = chrome.event(window, event);
        }
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.state == ElementState::Pressed
            && !event.repeat
            && event.physical_key == PhysicalKey::Code(winit::keyboard::KeyCode::Tab)
            && self.session.modifiers.state().control_key()
        {
            if let Some(id) = deck.model.cycle(self.session.modifiers.state().shift_key()) {
                self.workspace_select(&mut deck, id, event_loop);
            }
            consumed = true;
        }
        let interactive = deck
            .active
            .and_then(|id| deck.model.tab(id))
            .is_some_and(|tab| tab.spec.profile.interactive);
        let editing = deck.chrome.as_ref().is_some_and(Chrome::editing);
        let is_input = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        );
        let intercept = is_input && (consumed || editing || !interactive);
        if intercept {
            self.release(event_loop);
            self.session.pointer = false;
            self.session.pointer_point = None;
        }
        let clipboard = deck
            .active
            .and_then(|id| deck.model.tab(id))
            .is_some_and(|tab| tab.spec.profile.clipboard);
        lock(&self.session.handle.state).clipboard.focus(
            clipboard
                && !editing
                && self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.has_focus()),
        );
        self.workspace = Some(deck);
        intercept
    }

    fn workspace_select(&mut self, deck: &mut Deck, id: TabId, event_loop: &ActiveEventLoop) {
        if deck.active == Some(id) {
            return;
        }
        let Some(next) = deck.inactive.remove(&id) else {
            return;
        };
        self.release(event_loop);
        {
            let mut state = lock(&self.session.handle.state);
            state.clipboard.focus(false);
            state.occluded = true;
            state.native_window = None;
            state.submission_debt.hidden();
        }
        let previous = std::mem::replace(&mut self.session, next);
        if let Some(old) = deck.active
            && deck.model.tab(old).is_some()
        {
            deck.inactive.insert(old, previous);
        }
        deck.active = Some(id);
        let _ = deck.model.select(id);
        self.session.modifiers = Default::default();
        self.session.pointer = false;
        self.session.pointer_point = None;
        let mut state = lock(&self.session.handle.state);
        state.occluded = false;
        state.clipboard.focus(
            self.window.as_ref().is_some_and(|w| w.has_focus())
                && !deck.chrome.as_ref().is_some_and(Chrome::editing),
        );
        drop(state);
        if let Some(gpu) = &mut self.gpu {
            gpu.clear_picture();
        }
        self.last_window_status = None;
    }

    fn workspace_open(&mut self, deck: &mut Deck, spec: TabSpec, event_loop: &ActiveEventLoop) {
        let (id, fresh) = match deck.model.open(spec) {
            Ok(value) => value,
            Err(error) => {
                deck.message = error.to_string();
                return;
            }
        };
        if fresh {
            let tab = deck.model.tab(id).expect("newly inserted tab").clone();
            let (mut session, view, input) =
                SessionView::new(tab.spec.display, self.session.handle.proxy.clone());
            session.tab = Some(id);
            view.label(format!(
                "RDS · {} · Display {}",
                tab.spec.label, tab.spec.display
            ));
            if deck
                .events
                .try_send(WorkspaceEvent::Start { tab, view, input })
                .is_err()
            {
                let _ = deck.model.close(id);
                if let Some(previous) = deck.active {
                    let _ = deck.model.select(previous);
                }
                deck.message = "Connection queue is full; try again".into();
                return;
            }
            deck.inactive.insert(id, session);
        }
        self.workspace_select(deck, id, event_loop);
        if let Some(chrome) = &mut deck.chrome {
            chrome.connected();
        }
    }

    fn workspace_close(&mut self, deck: &mut Deck, id: TabId, event_loop: &ActiveEventLoop) {
        if deck.active == Some(id) {
            self.release(event_loop);
        }
        if deck.model.close(id).is_err() {
            return;
        }
        deck.inactive.remove(&id); // Sender drop closes this session's input.
        let _ = deck.events.try_send(WorkspaceEvent::Stop(id));
        if deck.active == Some(id) {
            if let Some(next) = deck.model.active() {
                self.workspace_select(deck, next, event_loop);
            } else {
                let (blank, _, _) = SessionView::new(0, self.session.handle.proxy.clone());
                self.session = blank;
                self.session.handle.status("Ready");
                deck.active = None;
                if let Some(gpu) = &mut self.gpu {
                    gpu.clear_picture();
                }
            }
        }
    }

    pub(super) fn workspace_actions(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut deck) = self.workspace.take() else {
            return;
        };
        let actions = deck
            .chrome
            .as_mut()
            .map(|c| std::mem::take(&mut c.actions))
            .unwrap_or_default();
        for action in actions {
            match action {
                Action::Select(id) => self.workspace_select(&mut deck, id, event_loop),
                Action::Close(id) => self.workspace_close(&mut deck, id, event_loop),
                Action::Open(spec) => self.workspace_open(&mut deck, spec, event_loop),
                Action::OpenAll(specs) => {
                    let mut trial = deck.model.clone();
                    let validation = specs
                        .iter()
                        .try_for_each(|spec| trial.open(spec.clone()).map(|_| ()));
                    if let Err(error) = validation {
                        deck.message = error.to_string();
                    } else {
                        for spec in specs {
                            self.workspace_open(&mut deck, spec, event_loop);
                        }
                    }
                }
                Action::Move(id, position) => {
                    let _ = deck.model.move_tab(id, position);
                }
                Action::Inspect { target, grant_file } => {
                    deck.message = "Loading displays…".into();
                    if deck
                        .events
                        .try_send(WorkspaceEvent::Inspect { target, grant_file })
                        .is_err()
                    {
                        deck.message = "Connection queue is full; try again".into();
                    }
                }
                Action::Configure(id, profile) => {
                    if deck.model.configure(id, profile).is_ok() {
                        self.workspace_restart(&mut deck, id, event_loop);
                    }
                }
                Action::Reconnect(id) => {
                    if let Some(tab) = deck.model.tab(id) {
                        let profile = tab.spec.profile.clone();
                        if deck.model.configure(id, profile).is_ok() {
                            self.workspace_restart(&mut deck, id, event_loop);
                        }
                    }
                }
                Action::Save => {
                    let tabs = deck
                        .model
                        .tabs()
                        .iter()
                        .map(|tab| tab.spec.clone())
                        .collect();
                    if deck.events.try_send(WorkspaceEvent::Save { tabs }).is_err() {
                        deck.message = "Save queue is full; try again".into();
                    }
                }
            }
        }
        self.workspace = Some(deck);
    }

    fn workspace_restart(&mut self, deck: &mut Deck, id: TabId, event_loop: &ActiveEventLoop) {
        let Some(tab) = deck.model.tab(id).cloned() else {
            return;
        };
        let (mut new, view, input) =
            SessionView::new(tab.spec.display, self.session.handle.proxy.clone());
        new.tab = Some(id);
        view.label(format!(
            "RDS · {} · Display {}",
            tab.spec.label, tab.spec.display
        ));
        if deck.active == Some(id) {
            self.release(event_loop);
            self.session = new;
            if let Some(gpu) = &mut self.gpu {
                gpu.clear_picture();
            }
        } else {
            deck.inactive.insert(id, new);
        }
        if deck
            .events
            .try_send(WorkspaceEvent::Start { tab, view, input })
            .is_err()
        {
            self.workspace_close(deck, id, event_loop);
            deck.message = "Connection queue is full; reopen this display".into();
        }
    }
}
