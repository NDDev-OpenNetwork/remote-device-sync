use rds_core::InputKind;
use winit::keyboard::KeyCode;

/// Translate a local Command paste while restoring every physically held key.
/// Control brackets Super release/restore so Super is never released alone.
pub(super) fn command_paste_chord(held: &std::collections::BTreeSet<u32>) -> Vec<InputKind> {
    command_chord(held, 47)
}

/// Copy/cut/paste use the remote desktop's Control shortcuts on macOS,
/// preserving Shift for terminal Ctrl+Shift+C/V without replaying Super.
pub(super) fn command_chord(held: &std::collections::BTreeSet<u32>, key: u32) -> Vec<InputKind> {
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
    chord.push(InputKind::KeyDown { code: key });
    chord.push(InputKind::KeyUp { code: key });
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

/// Replay only the original paste gesture after a clipboard handoff. Shift is
/// captured at that gesture; the physical modifier state may since have changed.
pub(super) fn deferred_paste_chord(
    held: &std::collections::BTreeSet<u32>,
    shift: bool,
) -> Vec<InputKind> {
    let mut effective = held.clone();
    let mut before = vec![];
    let mut after = vec![];
    if shift && !held.contains(&42) && !held.contains(&54) {
        effective.insert(42);
        before.push(InputKind::KeyDown { code: 42 });
        after.push(InputKind::KeyUp { code: 42 });
    } else if !shift {
        for code in [42, 54] {
            if effective.remove(&code) {
                before.push(InputKind::KeyUp { code });
                after.push(InputKind::KeyDown { code });
            }
        }
    }
    before.extend(command_paste_chord(&effective));
    before.extend(after);
    before
}

#[cfg(test)]
mod command_paste_tests {
    use super::*;
    #[test]
    fn delayed_paste_preserves_original_shift_and_restores_current_modifiers() {
        for held in [vec![], vec![29], vec![125], vec![42, 54, 97, 126]] {
            for shift in [false, true] {
                let mut pressed: std::collections::BTreeSet<u32> = held.iter().copied().collect();
                let original = pressed.clone();
                let mut pastes = 0;
                for event in deferred_paste_chord(&original, shift) {
                    match event {
                        InputKind::KeyDown { code } => {
                            assert!(pressed.insert(code), "duplicate key press");
                            if code == 47 {
                                pastes += 1;
                                assert!(pressed.contains(&29) || pressed.contains(&97));
                                assert!(!pressed.contains(&125) && !pressed.contains(&126));
                                assert_eq!(pressed.contains(&42) || pressed.contains(&54), shift);
                            }
                        }
                        InputKind::KeyUp { code } => {
                            assert!(pressed.remove(&code));
                        }
                        _ => panic!("unexpected pointer event"),
                    }
                }
                assert_eq!(pastes, 1);
                assert_eq!(pressed, original);
            }
        }
    }
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
    /// One fitted content rectangle for both GPU presentation and hit testing.
    /// Panels may consume the entire window at small sizes; then input and
    /// remote presentation are empty rather than extending outside the surface.
    pub fn content(
        width: u32,
        height: u32,
        remote_width: u32,
        remote_height: u32,
        area: Option<[f64; 4]>,
    ) -> Self {
        let [x, y, w, h] = area.unwrap_or([0., 0., f64::from(width), f64::from(height)]);
        if [x, y, w, h].iter().any(|v| !v.is_finite()) || remote_width == 0 || remote_height == 0 {
            return Self {
                x: 0.,
                y: 0.,
                width: 0.,
                height: 0.,
            };
        }
        let left = x.clamp(0., f64::from(width));
        let top = y.clamp(0., f64::from(height));
        let right = (x + w.max(0.)).clamp(left, f64::from(width));
        let bottom = (y + h.max(0.)).clamp(top, f64::from(height));
        let mut viewport = Self::new(
            (right - left) as u32,
            (bottom - top) as u32,
            remote_width,
            remote_height,
        );
        viewport.x += left;
        viewport.y += top;
        viewport
    }
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

/// A modifier may be reported both as physical input and as a flags update.
/// Apply one transition only; native repeats are handled explicitly upstream.
pub(super) fn key_transition(
    held: &mut std::collections::BTreeSet<u32>,
    code: u32,
    pressed: bool,
) -> bool {
    if pressed {
        held.insert(code)
    } else {
        held.remove(&code)
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
    fn workspace_panels_and_tiny_windows_share_presentation_and_input_bounds() {
        let view = Viewport::content(1200, 800, 1920, 1080, Some([0., 100., 1200., 680.]));
        assert!(view.pointer(600., 50., 1920, 1080).is_none());
        assert!(view.pointer(600., 790., 1920, 1080).is_none());
        let center = view
            .pointer(
                view.x + view.width / 2.,
                view.y + view.height / 2.,
                1920,
                1080,
            )
            .unwrap();
        assert_eq!(center, (960., 540.));
        assert!(view.x + view.width <= 1200. && view.y + view.height <= 800.);
        let tiny = Viewport::content(30, 20, 1920, 1080, Some([0., 100., 30., 0.]));
        assert_eq!(tiny.height, 0.);
        assert!(tiny.pointer(10., 10., 1920, 1080).is_none());
        let invalid = Viewport::content(1200, 800, 1920, 1080, Some([f64::NAN, 0., 10., 10.]));
        assert!(invalid.pointer(1., 1., 1920, 1080).is_none());
    }
    #[test]
    fn physical_and_flags_notifications_preserve_one_alt_shift_chord() {
        for physical_first in [false, true] {
            let mut held = std::collections::BTreeSet::new();
            let mut output = Vec::new();
            let mut flags = winit::keyboard::ModifiersState::empty();
            for (code, flag, pressed) in [
                (56, winit::keyboard::ModifiersState::ALT, true),
                (42, winit::keyboard::ModifiersState::SHIFT, true),
                (42, winit::keyboard::ModifiersState::SHIFT, false),
                (56, winit::keyboard::ModifiersState::ALT, false),
            ] {
                flags.set(flag, pressed);
                for physical in [physical_first, !physical_first] {
                    let changes = if physical {
                        vec![(code, pressed)]
                    } else {
                        modifier_changes(&held, flags.into())
                    };
                    for (code, pressed) in changes {
                        if key_transition(&mut held, code, pressed) {
                            output.push((code, pressed));
                        }
                    }
                }
            }
            assert_eq!(output, [(56, true), (42, true), (42, false), (56, false)]);
            assert!(held.is_empty());
        }
    }

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
