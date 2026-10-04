use rds_core::InputKind;
use winit::keyboard::KeyCode;

/// Translate a local Command paste while restoring every physically held key.
/// Control brackets Super release/restore so Super is never released alone.
pub(super) fn command_paste_chord(held: &std::collections::BTreeSet<u32>) -> Vec<InputKind> {
    let add_control = !held.contains(&29) && !held.contains(&97);
    let mut chord = Vec::with_capacity(8);
    if add_control {
        chord.push(InputKind::KeyDown { code: 29 });
    }
    for code in [125, 126] {
        if held.contains(&code) {
            chord.push(InputKind::KeyUp { code });
        }
    }
    chord.push(InputKind::KeyDown { code: 47 });
    chord.push(InputKind::KeyUp { code: 47 });
    for code in [125, 126] {
        if held.contains(&code) {
            chord.push(InputKind::KeyDown { code });
        }
    }
    if add_control {
        chord.push(InputKind::KeyUp { code: 29 });
    }
    chord
}

#[cfg(test)]
mod command_paste_tests {
    use super::*;
    #[test]
    fn command_paste_restores_held_modifiers_and_never_pastes_with_super_held() {
        for held in [vec![125], vec![126], vec![42, 125], vec![97, 125, 126]] {
            let mut pressed: std::collections::BTreeSet<u32> = held.into_iter().collect();
            let original = pressed.clone();
            let mut presses = 0;
            for event in command_paste_chord(&original) {
                match event {
                    InputKind::KeyDown { code } => {
                        if code == 47 {
                            assert!(pressed.contains(&29) || pressed.contains(&97));
                            assert!(!pressed.contains(&125) && !pressed.contains(&126));
                            presses += 1;
                        }
                        assert!(pressed.insert(code), "duplicate synthetic key down");
                    }
                    InputKind::KeyUp { code } => {
                        if code == 125 || code == 126 {
                            assert!(pressed.contains(&29) || pressed.contains(&97));
                        }
                        assert!(pressed.remove(&code), "release without held ownership");
                    }
                    _ => panic!("paste chord must only contain key transitions"),
                }
            }
            assert_eq!(presses, 1);
            assert_eq!(pressed, original);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Viewport {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Viewport {
    pub fn new(width: u32, height: u32, remote_width: u32, remote_height: u32) -> Self {
        let scale = (f64::from(width) / f64::from(remote_width.max(1)))
            .min(f64::from(height) / f64::from(remote_height.max(1)));
        let w = f64::from(remote_width) * scale;
        let h = f64::from(remote_height) * scale;
        Self {
            x: (f64::from(width) - w) / 2.,
            y: (f64::from(height) - h) / 2.,
            width: w,
            height: h,
        }
    }
    pub fn pointer(&self, x: f64, y: f64, width: u32, height: u32) -> Option<(f64, f64)> {
        if !x.is_finite()
            || !y.is_finite()
            || self.width <= 0.
            || self.height <= 0.
            || x < self.x
            || y < self.y
            || x >= self.x + self.width
            || y >= self.y + self.height
        {
            return None;
        }
        Some((
            ((x - self.x) / self.width * f64::from(width)).min(f64::from(width.saturating_sub(1))),
            ((y - self.y) / self.height * f64::from(height))
                .min(f64::from(height.saturating_sub(1))),
        ))
    }
}

/// Reconcile native modifier flags when a press happened before focus, or
/// the OS reports flags without a separate physical modifier key event.
/// Unknown sides retain a known held side; otherwise use the left key.
pub(super) fn modifier_changes(
    held: &std::collections::BTreeSet<u32>,
    modifiers: winit::event::Modifiers,
) -> Vec<(u32, bool)> {
    use winit::keyboard::ModifiersKeyState::Pressed;
    let state = modifiers.state();
    let groups = [
        (
            42,
            54,
            state.shift_key(),
            modifiers.lshift_state(),
            modifiers.rshift_state(),
        ),
        (
            29,
            97,
            state.control_key(),
            modifiers.lcontrol_state(),
            modifiers.rcontrol_state(),
        ),
        (
            56,
            100,
            state.alt_key(),
            modifiers.lalt_state(),
            modifiers.ralt_state(),
        ),
        (
            125,
            126,
            state.super_key(),
            modifiers.lsuper_state(),
            modifiers.rsuper_state(),
        ),
    ];
    let mut changes = Vec::with_capacity(8);
    for (left, right, active, lstate, rstate) in groups {
        let (mut l, mut r) = (lstate == Pressed, rstate == Pressed);
        if !active {
            (l, r) = (false, false);
        } else if !l && !r {
            (l, r) = (held.contains(&left), held.contains(&right));
            if !l && !r {
                l = true;
            }
        }
        for (code, pressed) in [(left, l), (right, r)] {
            if held.contains(&code) != pressed {
                changes.push((code, pressed));
            }
        }
    }
    changes.sort_by_key(|(_, pressed)| *pressed); // release before changing sides
    changes
}

/// Physical key mapping to the remote protocol's Linux evdev vocabulary.
/// Layout/IME composition stays with the target desktop.
pub(super) fn evdev(code: KeyCode) -> Option<u32> {
    use KeyCode::*;
    Some(match code {
        Escape => 1,
        Digit1 => 2,
        Digit2 => 3,
        Digit3 => 4,
        Digit4 => 5,
        Digit5 => 6,
        Digit6 => 7,
        Digit7 => 8,
        Digit8 => 9,
        Digit9 => 10,
        Digit0 => 11,
        Minus => 12,
        Equal => 13,
        Backspace => 14,
        Tab => 15,
        KeyQ => 16,
        KeyW => 17,
        KeyE => 18,
        KeyR => 19,
        KeyT => 20,
        KeyY => 21,
        KeyU => 22,
        KeyI => 23,
        KeyO => 24,
        KeyP => 25,
        BracketLeft => 26,
        BracketRight => 27,
        Enter => 28,
        ControlLeft => 29,
        KeyA => 30,
        KeyS => 31,
        KeyD => 32,
        KeyF => 33,
        KeyG => 34,
        KeyH => 35,
        KeyJ => 36,
        KeyK => 37,
        KeyL => 38,
        Semicolon => 39,
        Quote => 40,
        Backquote => 41,
        ShiftLeft => 42,
        Backslash => 43,
        KeyZ => 44,
        KeyX => 45,
        KeyC => 46,
        KeyV => 47,
        KeyB => 48,
        KeyN => 49,
        KeyM => 50,
        Comma => 51,
        Period => 52,
        Slash => 53,
        ShiftRight => 54,
        NumpadMultiply => 55,
        AltLeft => 56,
        Space => 57,
        CapsLock => 58,
        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        NumLock => 69,
        ScrollLock => 70,
        Numpad7 => 71,
        Numpad8 => 72,
        Numpad9 => 73,
        NumpadSubtract => 74,
        Numpad4 => 75,
        Numpad5 => 76,
        Numpad6 => 77,
        NumpadAdd => 78,
        Numpad1 => 79,
        Numpad2 => 80,
        Numpad3 => 81,
        Numpad0 => 82,
        NumpadDecimal => 83,
        IntlBackslash => 86,
        F11 => 87,
        F12 => 88,
        NumpadEnter => 96,
        ControlRight => 97,
        NumpadDivide => 98,
        PrintScreen => 99,
        AltRight => 100,
        Home => 102,
        ArrowUp => 103,
        PageUp => 104,
        ArrowLeft => 105,
        ArrowRight => 106,
        End => 107,
        ArrowDown => 108,
        PageDown => 109,
        Insert => 110,
        Delete => 111,
        Pause => 119,
        SuperLeft => 125,
        SuperRight => 126,
        ContextMenu => 127,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn option_flags_without_a_key_event_supply_alt_and_release_it() {
        let held = std::collections::BTreeSet::new();
        let alt = winit::event::Modifiers::from(winit::keyboard::ModifiersState::ALT);
        assert_eq!(modifier_changes(&held, alt), vec![(56, true)]);
        let held = std::collections::BTreeSet::from([56, 105]);
        assert_eq!(modifier_changes(&held, alt), vec![]);
        assert_eq!(
            modifier_changes(&held, Default::default()),
            vec![(56, false)]
        );
        assert_eq!(evdev(KeyCode::AltLeft), Some(56));
        assert_eq!(evdev(KeyCode::AltRight), Some(100));
        assert_eq!(evdev(KeyCode::ArrowUp), Some(103));
    }

    #[test]
    fn unknown_modifier_sides_preserve_right_keys_and_release_both_on_focus_reset() {
        let held = std::collections::BTreeSet::from([97, 100, 126]);
        let flags = winit::keyboard::ModifiersState::CONTROL
            | winit::keyboard::ModifiersState::ALT
            | winit::keyboard::ModifiersState::SUPER;
        assert_eq!(modifier_changes(&held, flags.into()), vec![]);
        assert_eq!(
            modifier_changes(&held, Default::default()),
            vec![(97, false), (100, false), (126, false)]
        );
    }

    #[test]
    fn letterboxing_and_scaled_video_preserve_remote_coordinates() {
        let v = Viewport::new(1000, 1000, 1920, 1080);
        assert!(v.pointer(100., 100., 1920, 1080).is_none());
        assert_eq!(v.pointer(500., 500., 1920, 1080), Some((960., 540.)));
        assert!(v.pointer(f64::NAN, 500., 1920, 1080).is_none());
        assert!(v.pointer(1000., 500., 1920, 1080).is_none());
        assert_eq!(evdev(KeyCode::ControlRight), Some(97));
        assert_eq!(evdev(KeyCode::KeyA), Some(30));
    }
}
