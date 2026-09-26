//! Run only on a disposable Xvfb server; these tests inject real native input.
#![cfg(all(target_os = "linux", feature = "x11"))]

use rds_core::{InputEvent, InputKind};
use rds_desktop::{InputSink, input::x11::XtestInput};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, KeyButMask};
use x11rb::rust_connection::RustConnection;

fn event(kind: InputKind) -> InputEvent {
    InputEvent {
        seq: 0,
        event_ts_ms: 0,
        display_id: 0,
        kind,
    }
}

fn observer() -> (RustConnection, u32) {
    let (conn, _) = RustConnection::connect(None).expect("dedicated X11 server required");
    let root = conn.setup().roots[0].root;
    (conn, root)
}

#[test]
#[ignore = "requires a dedicated Xvfb server; injects native input"]
fn native_evdev_keyboard_mapping() {
    let (observer, _) = observer();
    let mut sink = XtestInput::new().unwrap();
    sink.inject(&event(InputKind::KeyDown { code: 30 }))
        .unwrap(); // KEY_A
    let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
    sink.inject(&event(InputKind::KeyUp { code: 30 })).unwrap();
    assert_ne!(
        keys[38 / 8] & (1 << (38 % 8)),
        0,
        "evdev KEY_A must become X keycode 38"
    );
}

#[test]
#[ignore = "requires a dedicated Xvfb server; injects native input"]
fn native_evdev_button_mapping() {
    let (observer, root) = observer();
    let mut sink = XtestInput::new().unwrap();
    for (button, expected) in [
        (0x110, KeyButMask::BUTTON1),
        (0x111, KeyButMask::BUTTON3),
        (0x112, KeyButMask::BUTTON2),
    ] {
        sink.inject(&event(InputKind::PointerButton {
            button,
            pressed: true,
        }))
        .unwrap();
        let mask = observer.query_pointer(root).unwrap().reply().unwrap().mask;
        sink.inject(&event(InputKind::PointerButton {
            button,
            pressed: false,
        }))
        .unwrap();
        assert!(
            mask.contains(expected),
            "evdev button {button} was not pressed"
        );
    }
}

#[test]
#[ignore = "requires a dedicated Xvfb server; injects native input"]
fn native_relative_motion_preserves_position() {
    let (observer, root) = observer();
    let mut sink = XtestInput::new().unwrap();
    sink.inject(&event(InputKind::PointerMove { x: 100.0, y: 100.0 }))
        .unwrap();
    sink.inject(&event(InputKind::PointerMotion { dx: 5.0, dy: -3.0 }))
        .unwrap();
    let pointer = observer.query_pointer(root).unwrap().reply().unwrap();
    assert_eq!((pointer.root_x, pointer.root_y), (105, 97));
}

#[test]
#[ignore = "requires a dedicated Xvfb server; injects native input"]
fn native_scroll_flushes_without_a_later_event() {
    use std::time::{Duration, Instant};
    use x11rb::protocol::{
        Event,
        xproto::{ChangeWindowAttributesAux, EventMask},
    };
    let (observer, root) = observer();
    observer
        .change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::BUTTON_PRESS),
        )
        .unwrap()
        .check()
        .unwrap();
    let mut sink = XtestInput::new().unwrap();
    for _ in 0..3 {
        sink.inject(&event(InputKind::Scroll { dx: 0.0, dy: 0.25 }))
            .unwrap();
        assert!(
            observer.poll_for_event().unwrap().is_none(),
            "fraction rounded into a full step"
        );
    }
    sink.inject(&event(InputKind::Scroll { dx: 0.0, dy: 0.25 }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        if let Some(Event::ButtonPress(button)) = observer.poll_for_event().unwrap() {
            assert_eq!(button.detail, 4);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "scroll was not delivered without a later input event"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "requires a dedicated Xvfb server; injects native input"]
fn native_drop_releases_owned_holds_and_rejects_bad_values() {
    use std::time::{Duration, Instant};
    let (observer, root) = observer();
    let mut sink = XtestInput::new().unwrap();
    for kind in [
        InputKind::KeyDown { code: u32::MAX },
        InputKind::PointerButton {
            button: -1,
            pressed: true,
        },
        InputKind::PointerMove {
            x: f64::NAN,
            y: 1.0,
        },
        InputKind::PointerMotion {
            dx: f64::INFINITY,
            dy: 0.0,
        },
        InputKind::Scroll {
            dx: 0.0,
            dy: f64::NAN,
        },
    ] {
        assert!(sink.inject(&event(kind)).is_err());
    }
    sink.inject(&event(InputKind::KeyDown { code: 30 }))
        .unwrap();
    sink.inject(&event(InputKind::PointerButton {
        button: 0x110,
        pressed: true,
    }))
    .unwrap();
    assert_ne!(
        observer.query_keymap().unwrap().reply().unwrap().keys[38 / 8] & (1 << (38 % 8)),
        0
    );
    assert!(
        observer
            .query_pointer(root)
            .unwrap()
            .reply()
            .unwrap()
            .mask
            .contains(KeyButMask::BUTTON1)
    );
    drop(sink);
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
        let buttons = observer.query_pointer(root).unwrap().reply().unwrap().mask;
        if keys[38 / 8] & (1 << (38 % 8)) == 0 && !buttons.contains(KeyButMask::BUTTON1) {
            break;
        }
        assert!(Instant::now() < deadline, "native holds survived sink drop");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "requires a dedicated two-screen Xvfb server; injects native input"]
fn native_display_scope_and_invalid_capture_fail_closed() {
    use rds_desktop::capture::x11::X11Capturer;
    let (observer, root) = observer();
    assert!(
        observer.setup().roots.len() >= 2,
        "two-screen fixture required"
    );
    assert!(XtestInput::for_display(u32::MAX).is_err());
    assert!(X11Capturer::new(u32::MAX).is_err());
    let mut first = XtestInput::new().unwrap();
    first
        .inject(&event(InputKind::PointerMove { x: 100.0, y: 100.0 }))
        .unwrap();
    let mut second = XtestInput::for_display(1).unwrap();
    let mut action = event(InputKind::PointerMove { x: 10.0, y: 10.0 });
    assert!(second.inject(&action).is_err(), "wrong display accepted");
    action.display_id = 1;
    action.kind = InputKind::PointerButton {
        button: 0x110,
        pressed: true,
    };
    assert!(
        second.inject(&action).is_err(),
        "button targeted another screen"
    );
    action.kind = InputKind::KeyDown { code: 30 };
    assert!(
        second.inject(&action).is_err(),
        "keyboard targeted another screen"
    );
    action.kind = InputKind::Scroll { dx: 0.0, dy: 0.25 };
    assert!(
        second.inject(&action).is_err(),
        "fractional scroll targeted another screen"
    );
    action.kind = InputKind::PointerMove { x: 10.0, y: 10.0 };
    second.inject(&action).unwrap();
    let other = observer.setup().roots[1].root;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    loop {
        let pointer = observer.query_pointer(other).unwrap().reply().unwrap();
        if pointer.same_screen && (pointer.root_x, pointer.root_y) == (10, 10) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pointer did not reach selected screen: {pointer:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    first
        .inject(&event(InputKind::PointerMove { x: 100.0, y: 100.0 }))
        .unwrap();
    assert!(
        observer
            .query_pointer(root)
            .unwrap()
            .reply()
            .unwrap()
            .same_screen
    );
}
