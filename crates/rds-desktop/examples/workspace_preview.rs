//! Isolated native tab/input preview. No agent, network, credentials or remote OS.
use rds_core::{DesktopControl, InputKind};
use rds_desktop::render::workspace::{DeviceView, TabProfile, TabSpec};
use rds_desktop::{
    RawFrame,
    render::{Viewer, ViewerInput, WorkspaceEvent, WorkspaceUpdate},
};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
                    sessions.insert(id, tokio::spawn(async move {
                        view.status("Connected · isolated preview");
                        view.display_extent(960, 540);
                        let mut tick = tokio::time::interval(Duration::from_millis(100));
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        let mut keys = BTreeSet::new();
                        let mut buttons = BTreeSet::new();
                        let mut controls = 0u64;
                        loop {
                            tokio::select! {
                                _ = tick.tick() => {
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
                                            InputKind::KeyDown { code } => { keys.insert(code); },
                                            InputKind::KeyUp { code } => { keys.remove(&code); },
                                            InputKind::PointerButton { button, pressed } => {
                                                if pressed { buttons.insert(button); } else { buttons.remove(&button); }
                                            }
                                            _ => {},
                                        }
                                        println!("tab={id} controls={controls} held_keys={} held_buttons={}", keys.len(), buttons.len());
                                    }
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
