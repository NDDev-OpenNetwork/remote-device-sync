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
fn delayed_key_release_does_not_generate_remote_typematic_repeats() {
    use std::time::{Duration, Instant};
    use x11rb::protocol::{
        Event,
        xkb::{BoolCtrl, ConnectionExt as _, Control, ID},
        xproto::{
            AutoRepeatMode, ChangeKeyboardControlAux, ChangeWindowAttributesAux, EventMask,
            InputFocus,
        },
    };
    let (observer, root) = observer();
    observer.xkb_use_extension(1, 0).unwrap().reply().unwrap();
    let original = observer
        .xkb_get_controls(ID::USE_CORE_KBD.into())
        .unwrap()
        .reply()
        .unwrap();
    let rate = |delay, interval| {
        observer
            .xkb_set_controls(
                ID::USE_CORE_KBD.into(),
                0u16.into(),
                0u16.into(),
                0u16.into(),
                0u16.into(),
                0u16.into(),
                0u16.into(),
                0u16.into(),
                0u16.into(),
                original.mouse_keys_dflt_btn,
                original.groups_wrap,
                original.access_x_option,
                BoolCtrl::REPEAT_KEYS,
                BoolCtrl::REPEAT_KEYS,
                Control::from(u32::from(BoolCtrl::REPEAT_KEYS)),
                delay,
                interval,
                original.slow_keys_delay,
                original.debounce_delay,
                original.mouse_keys_delay,
                original.mouse_keys_interval,
                original.mouse_keys_time_to_max,
                original.mouse_keys_max_speed,
                original.mouse_keys_curve,
                original.access_x_timeout,
                original.access_x_timeout_mask,
                original.access_x_timeout_values,
                original.access_x_timeout_options_mask,
                original.access_x_timeout_options_values,
                &original.per_key_repeat,
            )
            .unwrap()
            .check()
            .unwrap();
    };
    rate(40, 20);
    observer
        .change_keyboard_control(
            &ChangeKeyboardControlAux::new()
                .key(38u32)
                .auto_repeat_mode(AutoRepeatMode::ON),
        )
        .unwrap()
        .check()
        .unwrap();
    observer
        .change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
        )
        .unwrap()
        .check()
        .unwrap();
    observer
        .set_input_focus(InputFocus::POINTER_ROOT, root, x11rb::CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    let mut sink = XtestInput::new().unwrap();
    sink.inject(&event(InputKind::KeyDown { code: 30 }))
        .unwrap();
    std::thread::sleep(Duration::from_millis(160)); // delayed network KeyUp
    let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
    assert_ne!(
        keys[38 / 8] & (1 << (38 % 8)),
        0,
        "genuine key hold was released early"
    );
    sink.inject(&event(InputKind::KeyUp { code: 30 })).unwrap();
    let mut presses = 0;
    let deadline = Instant::now() + Duration::from_millis(50);
    while Instant::now() < deadline {
        if let Some(Event::KeyPress(key)) = observer.poll_for_event().unwrap() {
            if key.detail == 38 {
                presses += 1;
            }
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(
        presses, 1,
        "a delayed release manufactured extra letters on the server"
    );
    for _ in 0..3 {
        sink.inject(&event(InputKind::KeyDown { code: 30 }))
            .unwrap();
    }
    sink.inject(&event(InputKind::KeyUp { code: 30 })).unwrap();
    let mut repeats = 0;
    while let Some(e) = observer.poll_for_event().unwrap() {
        if matches!(e, Event::KeyPress(key) if key.detail == 38) {
            repeats += 1;
        }
    }
    assert_eq!(
        repeats, 3,
        "intentional client repeats were lost or duplicated"
    );
    let mut other = XtestInput::new().unwrap();
    sink.inject(&event(InputKind::KeyDown { code: 30 }))
        .unwrap();
    other
        .inject(&event(InputKind::KeyDown { code: 30 }))
        .unwrap();
    sink.inject(&event(InputKind::KeyUp { code: 30 })).unwrap();
    let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
    assert_ne!(
        keys[38 / 8] & (1 << (38 % 8)),
        0,
        "one controller released another's hold"
    );
    drop(other);
    let keys = observer.query_keymap().unwrap().reply().unwrap().keys;
    assert_eq!(
        keys[38 / 8] & (1 << (38 % 8)),
        0,
        "last controller retained the key after drop"
    );
    let keyboard = observer.get_keyboard_control().unwrap().reply().unwrap();
    assert_ne!(
        keyboard.auto_repeats[38 / 8] & (1 << (38 % 8)),
        0,
        "original native repeat setting was not restored"
    );
    drop(sink);
    rate(original.repeat_delay, original.repeat_interval);
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

#[test]
#[ignore = "requires a dedicated Xvfb server; injects native input"]
fn native_alt_navigation_resolves_the_actual_xkb_physical_map() {
    use std::time::{Duration, Instant};
    use x11rb::protocol::{
        Event,
        xkb::{ConnectionExt as _, ID, NameDetail},
        xproto::{CreateWindowAux, EventMask, InputFocus, WindowClass},
    };
    let (observer, root) = observer();
    observer.xkb_use_extension(1, 0).unwrap().reply().unwrap();
    let names = observer
        .xkb_get_names(ID::USE_CORE_KBD.into(), NameDetail::KEY_NAMES)
        .unwrap()
        .reply()
        .unwrap();
    let native_up = names
        .value_list
        .key_names
        .unwrap()
        .iter()
        .position(|key| {
            key.name
                .iter()
                .copied()
                .filter(|byte| *byte != 0 && *byte != b' ')
                .collect::<Vec<_>>()
                == b"UP"
        })
        .map(|offset| names.first_key as usize + offset)
        .unwrap() as u8;
    let previous = observer.get_input_focus().unwrap().reply().unwrap().focus;
    let window = observer.generate_id().unwrap();
    observer
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            20,
            20,
            200,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
        )
        .unwrap()
        .check()
        .unwrap();
    observer.map_window(window).unwrap().check().unwrap();
    observer
        .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    let mut sink = XtestInput::new().unwrap();
    sink.inject(&event(InputKind::KeyDown { code: 56 }))
        .unwrap();
    sink.inject(&event(InputKind::KeyDown { code: 103 }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(Event::KeyPress(key)) = observer.poll_for_event().unwrap()
            && key.detail == native_up
        {
            assert!(
                key.state.contains(KeyButMask::MOD1),
                "native Up did not carry Alt"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "wire Up never reached the native Up key"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    sink.inject(&event(InputKind::KeyUp { code: 103 })).unwrap();
    sink.inject(&event(InputKind::KeyUp { code: 56 })).unwrap();
    drop(sink);
    observer
        .set_input_focus(InputFocus::PARENT, previous, x11rb::CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    observer.destroy_window(window).unwrap().check().unwrap();
}
