//! A real EI socket pair; no compositor or ambient desktop receives input.
use super::input::Group;
use rds_core::{InputEvent, InputKind};
use reis::{PendingRequestResult, event::DeviceCapability};
use reis::{
    eis,
    handshake::EisHandshaker,
    request::{EisRequest, EisRequestConverter},
};
use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Key(u32, bool),
    Move(f32, f32),
}

fn server(
    socket: UnixStream,
    seen: Arc<Mutex<Vec<Seen>>>,
    pause: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
) {
    let context = eis::Context::new(socket).unwrap();
    let mut handshake = EisHandshaker::new(&context, 1);
    let mut converter = None;
    let mut keyboard = None;
    let mut advertised = false;
    let deadline = Instant::now() + Duration::from_secs(12);
    while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
        if context.read().is_err() {
            return;
        }
        while let Some(request) = context.pending_request() {
            let PendingRequestResult::Request(request) = request else {
                panic!("invalid fixture request");
            };
            if let Some(converter) = converter.as_mut() {
                EisRequestConverter::handle_request(converter, request).unwrap();
            } else if let Some(response) = handshake.handle_request(request).unwrap() {
                let ready = EisRequestConverter::new(&context, response, 1);
                let _ = ready.handle().add_seat(
                    Some("fixture"),
                    DeviceCapability::Keyboard
                        | DeviceCapability::PointerAbsolute
                        | DeviceCapability::Button
                        | DeviceCapability::Scroll,
                );
                converter = Some(ready);
            }
        }
        if let Some(converter) = converter.as_mut() {
            while let Some(request) = converter.next_request() {
                match request {
                    EisRequest::Bind(bind) if !advertised => {
                        advertised = true;
                        let key = bind.seat.add_device(
                            Some("keyboard"),
                            eis::device::DeviceType::Virtual,
                            DeviceCapability::Keyboard.into(),
                            |_| {},
                        );
                        key.resumed();
                        keyboard = Some(key);
                        let pointer = bind.seat.add_device(
                            Some("pointer"),
                            eis::device::DeviceType::Virtual,
                            DeviceCapability::PointerAbsolute
                                | DeviceCapability::Button
                                | DeviceCapability::Scroll,
                            |device| {
                                device.device().region_mapping_id("4k-region");
                                device.device().region(1920, 0, 2880, 1620, 4.0 / 3.0);
                                device.device().region_mapping_id("fhd-region");
                                device.device().region(0, 540, 1920, 1080, 1.0);
                            },
                        );
                        pointer.resumed();
                    }
                    EisRequest::KeyboardKey(event) => seen.lock().unwrap().push(Seen::Key(
                        event.key,
                        event.state == eis::keyboard::KeyState::Press,
                    )),
                    EisRequest::PointerMotionAbsolute(event) => seen
                        .lock()
                        .unwrap()
                        .push(Seen::Move(event.dx_absolute, event.dy_absolute)),
                    EisRequest::Disconnect => return,
                    _ => {}
                }
            }
            if pause.try_recv().is_ok() {
                let keyboard = keyboard.as_ref().unwrap();
                keyboard.paused();
                keyboard.resumed();
            }
        }
        context.flush().unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn event(display_id: u32, kind: InputKind) -> InputEvent {
    InputEvent {
        display_id,
        seq: 1,
        kind,
        event_ts_ms: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_monitor_regions_shared_key_holds_and_pause_invalidate_old_leases() {
    let (client, socket) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (pause, commands) = mpsc::sync_channel(1);
    let (recording, stopping) = (seen.clone(), stop.clone());
    let fixture =
        tokio::task::spawn_blocking(move || server(socket, recording, commands, stopping));
    let mappings = BTreeMap::from([
        (
            10,
            (
                "4k-region".into(),
                super::capture::Slot::fixture(10, (3840, 2160)),
            ),
        ),
        (
            20,
            (
                "fhd-region".into(),
                super::capture::Slot::fixture(20, (1920, 1080)),
            ),
        ),
    ]);
    let group = Group::start(client.into(), mappings).unwrap();
    until(|| group.ready.load(Ordering::Acquire)).await;
    let mut first = group.sink(10, (3840, 2160)).unwrap();
    let mut second = group.sink(20, (1920, 1080)).unwrap();
    first
        .inject(&event(
            10,
            InputKind::PointerMove {
                x: 1920.0,
                y: 1080.0,
            },
        ))
        .unwrap();
    second
        .inject(&event(20, InputKind::PointerMove { x: 960.0, y: 540.0 }))
        .unwrap();
    assert!(
        first
            .inject(&event(20, InputKind::KeyDown { code: 30 }))
            .is_err()
    );
    assert!(
        first
            .inject(&event(
                10,
                InputKind::PointerMove {
                    x: f64::NAN,
                    y: 0.0
                }
            ))
            .is_err()
    );
    first
        .inject(&event(10, InputKind::KeyDown { code: 30 }))
        .unwrap();
    second
        .inject(&event(20, InputKind::KeyDown { code: 30 }))
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            Seen::Move(3360.0, 810.0),
            Seen::Move(960.0, 1080.0),
            Seen::Key(30, true)
        ]
    );
    drop(first);
    second
        .inject(&event(20, InputKind::KeyUp { code: 30 }))
        .unwrap();
    until(|| seen.lock().unwrap().last() == Some(&Seen::Key(30, false))).await;
    second
        .inject(&event(20, InputKind::KeyDown { code: 31 }))
        .unwrap();
    let era = group.era.load(Ordering::Acquire);
    pause.send(()).unwrap();
    until(|| group.era.load(Ordering::Acquire) != era).await;
    assert!(
        second
            .inject(&event(20, InputKind::KeyDown { code: 32 }))
            .is_err()
    );
    drop(second);
    until(|| group.ready.load(Ordering::Acquire)).await;
    let mut new = group.sink(10, (3840, 2160)).unwrap();
    new.inject(&event(10, InputKind::KeyDown { code: 33 }))
        .unwrap();
    drop(new);
    until(|| seen.lock().unwrap().last() == Some(&Seen::Key(33, false))).await;
    assert!(!seen.lock().unwrap().contains(&Seen::Key(32, true)));
    group.close().await.unwrap();
    stop.store(true, Ordering::Release);
    fixture.await.unwrap();
}

#[tokio::test]
async fn silent_eis_peer_cannot_hold_handshake_admission_forever() {
    let (client, _silent) = UnixStream::pair().unwrap();
    let group = Group::start(client.into(), BTreeMap::new()).unwrap();
    until(|| group.stopped()).await;
    assert!(!group.ready.load(Ordering::Acquire));
    assert!(group.close().await.is_err());
}
