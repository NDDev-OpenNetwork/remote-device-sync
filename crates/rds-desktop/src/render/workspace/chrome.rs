//! Local workspace widgets; they never send remote input or perform network I/O.
use super::{TabId, TabProfile, TabSpec, VideoSize, WorkspaceModel};

#[derive(Clone)]
pub struct DeviceView {
    pub key: String,
    pub label: String,
    pub displays: Vec<rds_core::DisplayInfo>,
}

impl DeviceView {
    pub fn validate(&self) -> Result<(), super::WorkspaceError> {
        TabSpec {
            device: self.key.clone(),
            label: self.label.clone(),
            display: 0,
            profile: TabProfile::default(),
        }
        .validate()?;
        if self.displays.len() > 32 {
            return Err(super::WorkspaceError::Capacity);
        }
        let mut ids = std::collections::HashSet::new();
        if self
            .displays
            .iter()
            .any(|display| !ids.insert(display.index))
        {
            return Err(super::WorkspaceError::InvalidDevice);
        }
        Ok(())
    }
}

pub(crate) enum Action {
    Select(TabId),
    Close(TabId),
    Move(TabId, usize),
    Open(TabSpec),
    OpenAll(Vec<TabSpec>),
    Inspect { target: String, grant_file: String },
    Configure(TabId, TabProfile),
    Reconnect(TabId),
    Save,
}

pub(crate) struct UiFrame {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures: egui::TexturesDelta,
    pub pixels_per_point: f32,
    /// Physical-pixel desktop content area, shared with pointer mapping.
    pub content: egui::Rect,
}

pub(crate) struct Chrome {
    context: egui::Context,
    input: egui_winit::State,
    pub frame: UiFrame,
    pub actions: Vec<Action>,
    pub message: String,
    connect_open: bool,
    target: String,
    grant_file: String,
    settings: Option<(TabId, TabProfile)>,
    about: bool,
}

impl Chrome {
    pub fn connected(&mut self) {
        self.connect_open = false;
    }
    pub fn new(window: &winit::window::Window) -> Self {
        let context = egui::Context::default();
        context.set_visuals(egui::Visuals::dark());
        context.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.item_spacing = egui::vec2(10., 8.);
            style.spacing.button_padding = egui::vec2(12., 7.);
        });
        let input = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(4096),
        );
        Self {
            context,
            input,
            frame: UiFrame {
                primitives: vec![],
                textures: Default::default(),
                pixels_per_point: window.scale_factor() as f32,
                content: egui::Rect::NOTHING,
            },
            actions: vec![],
            message: String::new(),
            connect_open: false,
            target: String::new(),
            grant_file: String::new(),
            settings: None,
            about: false,
        }
    }

    pub fn event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        let response = self.input.on_window_event(window, event);
        if response.repaint {
            window.request_redraw();
        }
        response.consumed
    }

    pub fn editing(&self) -> bool {
        self.connect_open
            || self.settings.is_some()
            || self.about
            || self.context.egui_wants_keyboard_input()
    }

    pub fn prepare(
        &mut self,
        window: &winit::window::Window,
        model: &WorkspaceModel,
        devices: &[DeviceView],
        status: &str,
    ) {
        let context = self.context.clone();
        let output = context.run_ui(self.input.take_egui_input(window), |root| {
            egui::Panel::top("workspace-tabs").show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("RDS").strong().size(20.));
                    ui.separator();
                    egui::ScrollArea::horizontal().id_salt("tab-scroll").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (index, tab) in model.tabs().iter().enumerate() {
                                let label = format!("{} · Display {}", tab.spec.label, tab.spec.display);
                                let response = ui.selectable_label(model.active() == Some(tab.id), label);
                                if response.clicked() { self.actions.push(Action::Select(tab.id)); }
                                response.context_menu(|ui| {
                                    if ui.button("Move left").clicked() {
                                        self.actions.push(Action::Move(tab.id, index.saturating_sub(1)));
                                        ui.close();
                                    }
                                    if ui.button("Move right").clicked() {
                                        self.actions.push(Action::Move(tab.id, index + 1)); ui.close();
                                    }
                                    if ui.button("Close display").clicked() {
                                        self.actions.push(Action::Close(tab.id)); ui.close();
                                    }
                                });
                                if ui.small_button("×").on_hover_text("Close this display").clicked() {
                                    self.actions.push(Action::Close(tab.id));
                                }
                            }
                        });
                    });
                    if ui.button("+ Connect").clicked() { self.connect_open = true; }
                });
                ui.horizontal(|ui| {
                    if let Some(tab) = model.active().and_then(|id| model.tab(id)) {
                        if ui.button("Settings").clicked() {
                            self.settings = Some((tab.id, tab.spec.profile.clone()));
                        }
                        if ui.button("Reconnect").clicked() { self.actions.push(Action::Reconnect(tab.id)); }
                        ui.label(format!("{} · {} FPS{}", video_label(tab.spec.profile.video_size),
                            tab.spec.profile.max_fps, if tab.spec.profile.interactive { "" } else { " · View only" }));
                    }
                    if ui.button("Save workspace").clicked() { self.actions.push(Action::Save); }
                    if ui.button("About").clicked() { self.about = true; }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak("Ctrl+Tab to switch");
                    });
                });
            });
            egui::Panel::bottom("workspace-status").show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(status);
                    if !self.message.is_empty() { ui.separator(); ui.label(&self.message); }
                });
            });
            let content = root.available_rect_before_wrap();
            self.frame.content = egui::Rect::from_min_max(
                content.min * context.pixels_per_point(), content.max * context.pixels_per_point());
            if model.tabs().is_empty() {
                egui::CentralPanel::default().show(root, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space((ui.available_height() * 0.3).max(20.));
                        ui.heading("Your remote workspace");
                        ui.label("Open displays from one or several computers. Each display stays connected in its own tab.");
                        ui.add_space(16.);
                        if ui.button("Connect a computer").clicked() { self.connect_open = true; }
                    });
                });
            }
            if self.connect_open {
                let mut open = true;
                egui::Window::new("Connect a computer").open(&mut open)
                    .default_width(520.).collapsible(false).show(&context, |ui| {
                        ui.label("Device name, endpoint ID or connection ticket");
                        ui.add(egui::TextEdit::singleline(&mut self.target).char_limit(8192).desired_width(f32::INFINITY));
                        ui.collapsing("Connection authorization", |ui| {
                            ui.label("Optional grant file on this computer");
                            ui.add(egui::TextEdit::singleline(&mut self.grant_file).char_limit(4096).desired_width(f32::INFINITY));
                        });
                        if ui.add_enabled(!self.target.trim().is_empty(), egui::Button::new("Find displays")).clicked() {
                            self.actions.push(Action::Inspect { target: self.target.trim().into(), grant_file: self.grant_file.trim().into() });
                        }
                        if !self.message.is_empty() { ui.label(&self.message); }
                        ui.separator();
                        egui::ScrollArea::vertical().max_height(300.).show(ui, |ui| {
                            for device in devices {
                                ui.group(|ui| {
                                    ui.strong(&device.label);
                                    if ui.button("Load displays").clicked() {
                                        self.actions.push(Action::Inspect { target: device.key.clone(), grant_file: String::new() });
                                    }
                                    if device.displays.is_empty() { ui.weak("Inspect the computer to load its displays"); }
                                    for display in &device.displays {
                                        ui.horizontal(|ui| {
                                            ui.label(format!("Display {} · {} × {}{}", display.index, display.width, display.height,
                                                if display.primary { " · Primary" } else { "" }));
                                            if ui.button("Open").clicked() {
                                                self.actions.push(Action::Open(TabSpec { device: device.key.clone(), label: device.label.clone(),
                                                    display: display.index, profile: TabProfile::default() }));
                                            }
                                        });
                                    }
                                    if device.displays.len() > 1 && ui.button("Open all displays").clicked() {
                                        self.actions.push(Action::OpenAll(device.displays.iter().map(|display|
                                            TabSpec { device: device.key.clone(), label: device.label.clone(),
                                                display: display.index, profile: TabProfile::default() }).collect()));
                                    }
                                });
                            }
                        });
                    });
                self.connect_open = open;
            }
            if let Some((id, mut profile)) = self.settings.clone() {
                let mut open = model.tab(id).is_some();
                let mut apply = false;
                egui::Window::new("Display settings").open(&mut open)
                    .default_width(360.).collapsible(false).show(&context, |ui| {
                        if let Some(tab) = model.tab(id) {
                            ui.strong(format!("{} · Display {}", tab.spec.label, tab.spec.display));
                        }
                        ui.add_space(8.);
                        egui::ComboBox::from_label("Resolution").selected_text(video_label(profile.video_size))
                            .show_ui(ui, |ui| {
                                for size in [VideoSize::Hd, VideoSize::FullHd, VideoSize::Native] {
                                    ui.selectable_value(&mut profile.video_size, size, video_label(size));
                                }
                            });
                        ui.horizontal(|ui| {
                            ui.label("Frame rate");
                            ui.add(egui::DragValue::new(&mut profile.max_fps).range(1..=240).suffix(" FPS"));
                        });
                        ui.checkbox(&mut profile.interactive, "Enable keyboard and mouse");
                        ui.checkbox(&mut profile.clipboard, "Share text clipboard when focused");
                        ui.checkbox(&mut profile.payload_receipts, "Confirm complete frame delivery");
                        ui.weak("Applying restarts this display only. Other tabs stay connected.");
                        apply = ui.button("Apply to this display").clicked();
                    });
                self.settings = if open && !apply { Some((id, profile.clone())) } else { None };
                if apply { self.actions.push(Action::Configure(id, profile)); }
            }
            if self.about {
                egui::Window::new("About RDS").open(&mut self.about).default_width(540.).show(&context, |ui| {
                    ui.heading("RDS remote workspace");
                    ui.label("Multiple computers and displays, through your local RDS agent.");
                    ui.label(concat!("Version ", env!("CARGO_PKG_VERSION")));
                    ui.separator();
                    ui.label("Embedded font copyright and license notices");
                    egui::ScrollArea::vertical().max_height(340.).show(ui, |ui| {
                        for (name, text) in FONT_NOTICES {
                            ui.collapsing(*name, |ui| { ui.label(*text); });
                        }
                    });
                });
            }
        });
        self.input
            .handle_platform_output(window, output.platform_output);
        self.frame.pixels_per_point = output.pixels_per_point;
        self.frame.textures.append(output.textures_delta);
        self.frame.primitives = context.tessellate(output.shapes, output.pixels_per_point);
    }
}

const FONT_NOTICES: &[(&str, &str)] = &[
    (
        "Ubuntu Light",
        include_str!("../../../assets/font-licenses/UFL.txt"),
    ),
    (
        "Noto Emoji",
        include_str!("../../../assets/font-licenses/OFL.txt"),
    ),
    (
        "Hack",
        include_str!("../../../assets/font-licenses/Hack-Regular.txt"),
    ),
    (
        "Emoji Icon",
        include_str!("../../../assets/font-licenses/emoji-icon-font-mit-license.txt"),
    ),
];

fn video_label(size: VideoSize) -> &'static str {
    match size {
        VideoSize::Hd => "HD",
        VideoSize::FullHd => "Full HD",
        VideoSize::Native => "Native resolution",
    }
}
