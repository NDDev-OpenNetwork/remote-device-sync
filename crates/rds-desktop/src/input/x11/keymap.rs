//! Physical XKB key names bridge evdev and legacy XFree86 core keycodes.
use crate::DesktopError;

pub(super) struct KeyMap([Option<u8>; 256]);
impl KeyMap {
    pub(super) fn from_names(first: u8, names: &[[u8; 4]]) -> Result<Self, DesktopError> {
        if usize::from(first) + names.len() > 256 {
            return Err(DesktopError::Input(
                "XKB key names exceed the core keycode range".into(),
            ));
        }
        let mut map = [None; 256];
        for (code, slot) in map.iter_mut().enumerate() {
            let Some(expected) = physical_name(code as u32) else {
                continue;
            };
            for (index, name) in names.iter().enumerate() {
                let mut name = *name;
                for byte in &mut name {
                    if *byte == 0 {
                        *byte = b' ';
                    }
                }
                if name == expected {
                    if slot.is_some() {
                        return Err(DesktopError::Input(
                            "ambiguous XKB physical key name".into(),
                        ));
                    }
                    *slot = Some((usize::from(first) + index) as u8);
                }
            }
        }
        Ok(Self(map))
    }
    pub(super) fn resolve(&self, code: u32) -> Result<u8, DesktopError> {
        usize::try_from(code)
            .ok()
            .and_then(|code| self.0.get(code))
            .copied()
            .flatten()
            .ok_or_else(|| {
                DesktopError::Input("evdev key has no physical mapping in the XKB keyboard".into())
            })
    }
    pub(super) fn len(&self) -> usize {
        self.0.iter().filter(|key| key.is_some()).count()
    }
}
fn numbered(prefix: [u8; 2], number: u32) -> [u8; 4] {
    [
        prefix[0],
        prefix[1],
        b'0' + (number / 10) as u8,
        b'0' + (number % 10) as u8,
    ]
}
fn physical_name(code: u32) -> Option<[u8; 4]> {
    Some(match code {
        1 => *b"ESC ",
        2..=13 => numbered(*b"AE", code - 1),
        14 => *b"BKSP",
        15 => *b"TAB ",
        16..=27 => numbered(*b"AD", code - 15),
        28 => *b"RTRN",
        29 => *b"LCTL",
        30..=40 => numbered(*b"AC", code - 29),
        41 => *b"TLDE",
        42 => *b"LFSH",
        43 => *b"BKSL",
        44..=53 => numbered(*b"AB", code - 43),
        54 => *b"RTSH",
        55 => *b"KPMU",
        56 => *b"LALT",
        57 => *b"SPCE",
        58 => *b"CAPS",
        59..=68 => numbered(*b"FK", code - 58),
        69 => *b"NMLK",
        70 => *b"SCLK",
        71 => *b"KP7 ",
        72 => *b"KP8 ",
        73 => *b"KP9 ",
        74 => *b"KPSU",
        75 => *b"KP4 ",
        76 => *b"KP5 ",
        77 => *b"KP6 ",
        78 => *b"KPAD",
        79 => *b"KP1 ",
        80 => *b"KP2 ",
        81 => *b"KP3 ",
        82 => *b"KP0 ",
        83 => *b"KPDL",
        86 => *b"LSGT",
        87 => *b"FK11",
        88 => *b"FK12",
        96 => *b"KPEN",
        97 => *b"RCTL",
        98 => *b"KPDV",
        99 => *b"PRSC",
        100 => *b"RALT",
        102 => *b"HOME",
        103 => *b"UP  ",
        104 => *b"PGUP",
        105 => *b"LEFT",
        106 => *b"RGHT",
        107 => *b"END ",
        108 => *b"DOWN",
        109 => *b"PGDN",
        110 => *b"INS ",
        111 => *b"DELE",
        119 => *b"PAUS",
        125 => *b"LWIN",
        126 => *b"RWIN",
        127 => *b"MENU",
        183..=194 => numbered(*b"FK", code - 170),
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_navigation_and_modifiers_never_become_print_screen() {
        let mut names = vec![[0; 4]; 120];
        for (key, name) in [
            (64, *b"LALT"),
            (98, *b"UP\0\0"),
            (100, *b"LEFT"),
            (102, *b"RGHT"),
            (104, *b"DOWN"),
            (109, *b"RCTL"),
            (111, *b"PRSC"),
            (113, *b"RALT"),
            (115, *b"LWIN"),
            (116, *b"RWIN"),
        ] {
            names[key - 8] = name;
        }
        let map = KeyMap::from_names(8, &names).unwrap();
        for (wire, native) in [
            (56, 64),
            (103, 98),
            (105, 100),
            (106, 102),
            (108, 104),
            (97, 109),
            (99, 111),
            (100, 113),
            (125, 115),
            (126, 116),
        ] {
            assert_eq!(map.resolve(wire).unwrap(), native);
        }
        assert_ne!(
            map.resolve(103).unwrap(),
            111,
            "Up must not inject Print Screen"
        );
    }
    #[test]
    fn evdev_names_preserve_the_standard_map_without_reading_layout_symbols() {
        let mut names = vec![[0; 4]; 248];
        for code in 1..=247 {
            if let Some(name) = physical_name(code) {
                names[code as usize] = name;
            }
        }
        let map = KeyMap::from_names(8, &names).unwrap();
        for code in [
            1, 16, 30, 44, 56, 87, 97, 99, 100, 103, 105, 106, 108, 125, 126,
        ] {
            assert_eq!(map.resolve(code).unwrap(), (code + 8) as u8);
        }
        assert!(map.resolve(0).is_err());
        assert!(map.resolve(u32::MAX).is_err());
    }
    #[test]
    fn malformed_missing_or_ambiguous_names_fail_without_guessing_a_key() {
        assert!(KeyMap::from_names(250, &[[0; 4]; 10]).is_err());
        assert!(KeyMap::from_names(8, &[*b"LALT", *b"LALT"]).is_err());
        assert!(KeyMap::from_names(8, &[]).unwrap().resolve(103).is_err());
    }
}
