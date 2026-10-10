//! Isolated native tab/input preview. No agent, network, credentials or remote OS.
use rds_core::{ClipboardFormat, DesktopControl, DesktopEvent, InputKind};
use rds_desktop::render::workspace::{DeviceView, TabProfile, TabSpec};
use rds_desktop::{
    RawFrame,
    render::{Viewer, ViewerInput, WorkspaceEvent, WorkspaceUpdate},
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct ClipboardProbe {
    latest: String,
    copies: u64,
    transfers: u64,
    pastes: u64,
    valid_pastes: u64,
    shifted_pastes: u64,
    tab_presses: u64,
    unexpected_text: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let clipboard_report = match args.as_slice() {
        [] => None,
        [flag, path] if flag == "--clipboard-report" => {
            if std::path::Path::new(path).exists() {
                return Err("report already exists".into());
            }
            Some(std::path::PathBuf::from(path))
        }
        _ => return Err("usage: workspace_preview [--clipboard-report NEW_PATH]".into()),
    };
    let clipboard_probe = clipboard_report.is_some();
    let probe = Arc::new(Mutex::new(ClipboardProbe::default()));
    let observations = probe.clone();
    let devices = vec![
        DeviceView {
            key: "alpha".into(),
            label: "Studio PC".into(),
            displays: vec![display(0), display(1)],
        },
        DeviceView {
            key: "beta".into(),
            label: "Build PC".into(),
            displays: vec![display(0)],
        },
    ];
    let tabs = devices
        .iter()
        .flat_map(|device| {
            device.displays.iter().map(|display| TabSpec {
                device: device.key.clone(),
                label: device.label.clone(),
                display: display.index,
                profile: TabProfile::default(),
            })
        })
        .collect();
    let (viewer, workspace, mut events) = Viewer::workspace(tabs, devices)?;
    if clipboard_probe {
        let workspace = workspace.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(180)).await;
            let _ = workspace.update(WorkspaceUpdate::CloseWindow);
        });
    }
    let actor = tokio::spawn(async move {
        let mut sessions = BTreeMap::<u64, tokio::task::JoinHandle<()>>::new();
        while let Some(event) = events.recv().await {
            match event {
                WorkspaceEvent::Start {
                    tab,
                    view,
                    mut input,
                } => {
                    if let Some(old) = sessions.remove(&tab.id.value()) {
                        old.abort();
                    }
                    let id = tab.id.value();
                    let probe = probe.clone();
                    sessions.insert(id, tokio::spawn(async move {
                        view.status("Connected · isolated preview");
                        view.display_extent(960, 540);
                        let mut tick = tokio::time::interval(Duration::from_millis(100));
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        let mut keys = BTreeSet::new();
                        let mut buttons = BTreeSet::new();
                        let mut controls = 0u64;
                        let mut pending_offer: Option<(u64, String, Instant)> = None;
                        let mut pending_reply: Option<(u64, String, Instant)> = None;
                        let mut offered: Option<(u64, String)> = None;
                        let mut assembly = rds_desktop::clipboard::Assembly::default();
                        let mut received: Option<String> = None;
                        loop {
                            tokio::select! {
                                _ = tick.tick() => {
                                    if pending_offer.as_ref().is_some_and(|(_,_,at)| Instant::now() >= *at) {
                                        let (transfer, text, _) = pending_offer.take().unwrap();
                                        view.clipboard_event(DesktopEvent::ClipboardOffer { id: transfer,
                                            format: ClipboardFormat::TextUtf8, bytes: text.len() as u32 });
                                        offered = Some((transfer, text));
                                    }
                                    if pending_reply.as_ref().is_some_and(|(_,_,at)| Instant::now() >= *at) {
                                        let (transfer, text, _) = pending_reply.take().unwrap();
                                        view.clipboard_event(DesktopEvent::ClipboardChunk { id: transfer,
                                            offset: 0, total: text.len() as u32, data: text.into_bytes() });
                                    }
                                    let color = match id % 3 { 0 => [86, 106, 33], 1 => [137, 75, 37], _ => [72, 50, 128] };
                                    let mut data = vec![0; 960 * 540 * 4];
                                    for (index, pixel) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                                        let bright = ((index / 960 / 60 + index % 960 / 60) % 2) as u8 * 12;
                                        pixel.copy_from_slice(&[color[0] + bright, color[1] + bright, color[2] + bright, 255]);
                                    }
                                    view.frame(RawFrame { width: 960, height: 540, stride: 3840, data: data.into() }, Instant::now());
                                }
                                event = input.recv() => match event {
                                    Some(ViewerInput::Control(DesktopControl::Input(event))) => {
                                        controls += 1;
                                        match event.kind {
                                            InputKind::KeyDown { code } => {
                                                keys.insert(code);
                                                if clipboard_probe && code == 15 { probe.lock().unwrap().tab_presses += 1; }
                                                if clipboard_probe && (keys.contains(&29) || keys.contains(&97)) {
                                                    if matches!(code,45|46) {
                                                        let mut state = probe.lock().unwrap();
                                                        state.copies += 1;
                                                        let text = format!("RDS synthetic clipboard tab {id} copy {}", state.copies);
                                                        state.latest = text.clone();
                                                        pending_offer = Some((state.copies, text, Instant::now()+Duration::from_secs(1)));
                                                    } else if code == 47 {
                                                        let mut state = probe.lock().unwrap();
                                                        state.pastes += 1;
                                                        if keys.contains(&42) || keys.contains(&54) { state.shifted_pastes += 1; }
                                                        if received.as_ref() == Some(&state.latest) && !state.latest.is_empty() {
                                                            state.valid_pastes += 1;
                                                        }
                                                        println!("tab={id} probe_pastes={} valid_pastes={}",state.pastes,state.valid_pastes);
                                                    }
                                                }
                                            },
                                            InputKind::KeyUp { code } => { keys.remove(&code); },
                                            InputKind::PointerButton { button, pressed } => {
                                                if pressed { buttons.insert(button); } else { buttons.remove(&button); }
                                            }
                                            _ => {},
                                        }
                                        println!("tab={id} controls={controls} held_keys={} held_buttons={}", keys.len(), buttons.len());
                                    }
                                    Some(ViewerInput::Control(DesktopControl::ClipboardRequest { id: transfer, .. })) if clipboard_probe => {
                                        if let Some((expected, text)) = offered.take() {
                                            assert_eq!(transfer, expected);
                                            pending_reply = Some((transfer, text, Instant::now()+Duration::from_secs(2)));
                                        }
                                    },
                                    Some(ViewerInput::Control(DesktopControl::ClipboardChunk { id: transfer, offset, total, data })) if clipboard_probe => {
                                        if let Some(text) = assembly.push(transfer,offset,total,data).expect("bounded fixture clipboard") {
                                            let mut state = probe.lock().unwrap();
                                            state.transfers += 1;
                                            if text != state.latest { state.unexpected_text += 1; }
                                            // Retain only known synthetic text, never an unrelated clipboard value.
                                            received = (text == state.latest).then_some(text);
                                            view.clipboard_ack(transfer,total);
                                            println!("tab={id} probe_transfers={} unexpected_text={}",state.transfers,state.unexpected_text);
                                        }
                                    },
                                    Some(ViewerInput::Control(_)) => {},
                                    Some(ViewerInput::Close) | None => break,
                                }
                            }
                        }
                        println!("tab={id} closed controls={controls} held_keys={} held_buttons={}", keys.len(), buttons.len());
                    }));
                }
                WorkspaceEvent::Stop(id) => {
                    // The view's input close owns normal termination; retain it
                    // until joined so the preview reports held-input state.
                    if let Some(task) = sessions.remove(&id.value()) {
                        let _ = task.await;
                    }
                }
                WorkspaceEvent::Save { tabs } => {
                    let _ = workspace.update(WorkspaceUpdate::Message(format!(
                        "Preview: {} tabs; no files written",
                        tabs.len()
                    )));
                }
                WorkspaceEvent::Inspect { .. } => {
                    let _ = workspace.update(WorkspaceUpdate::Message(
                        "Preview devices are already loaded; no network access".into(),
                    ));
                }
            }
        }
        for (_, task) in sessions {
            let _ = task.await;
        }
    });
    tokio::task::block_in_place(|| viewer.run())?;
    tokio::time::timeout(Duration::from_secs(3), actor).await??;
    if let Some(report) = clipboard_report {
        use std::io::Write;
        let state = observations.lock().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(report)?;
        writeln!(
            file,
            "{{\"copies\":{},\"transfers\":{},\"pastes\":{},\"valid_pastes\":{},\"unexpected_text\":{},\"shifted_pastes\":{},\"tab_presses\":{}}}",
            state.copies,
            state.transfers,
            state.pastes,
            state.valid_pastes,
            state.unexpected_text,
            state.shifted_pastes,
            state.tab_presses
        )?;
        if state.valid_pastes == 0
            || state.pastes != state.valid_pastes
            || state.unexpected_text != 0
        {
            return Err("clipboard gesture acceptance failed; inspect report".into());
        }
    }
    Ok(())
}

fn display(index: u32) -> rds_core::DisplayInfo {
    rds_core::DisplayInfo {
        index,
        width: 960,
        height: 540,
        primary: index == 0,
    }
}
