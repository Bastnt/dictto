//! Linux evdev key codes to the core's platform-neutral keys.
use dictto_core::keys::Key;
use evdev::{EventType, KeyCode};

pub const EV_KEY: u16 = EventType::KEY.0;

// Letters in key-code order: the evdev codes follow the QWERTY rows, not the alphabet.
const LETTER_ROWS: [(KeyCode, &[u8]); 3] = [
    (KeyCode::KEY_Q, b"QWERTYUIOP"),
    (KeyCode::KEY_A, b"ASDFGHJKL"),
    (KeyCode::KEY_Z, b"ZXCVBNM"),
];

pub fn from_evdev(code: u16) -> Option<Key> {
    let key = match KeyCode(code) {
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => Key::Ctrl,
        KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => Key::Alt,
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => Key::Shift,
        KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => Key::Super,
        KeyCode::KEY_SPACE => Key::Space,
        KeyCode::KEY_ENTER => Key::Enter,
        KeyCode::KEY_TAB => Key::Tab,
        KeyCode::KEY_BACKSPACE => Key::Backspace,
        KeyCode::KEY_0 => Key::Char(b'0'),
        _ => return function_key(code).or_else(|| digit(code)).or_else(|| letter(code)),
    };
    Some(key)
}

fn function_key(code: u16) -> Option<Key> {
    let offset = |first: KeyCode, count: u16| code.checked_sub(first.0).filter(|n| *n < count);
    if let Some(n) = offset(KeyCode::KEY_F1, 10) {
        return Some(Key::F(1 + n as u8));
    }
    if let Some(n) = offset(KeyCode::KEY_F11, 2) {
        return Some(Key::F(11 + n as u8));
    }
    offset(KeyCode::KEY_F13, 12).map(|n| Key::F(13 + n as u8))
}

fn digit(code: u16) -> Option<Key> {
    // KEY_1..KEY_9 are contiguous; KEY_0 follows KEY_9 and is matched separately.
    code.checked_sub(KeyCode::KEY_1.0)
        .filter(|n| *n < 9)
        .map(|n| Key::Char(b'1' + n as u8))
}

fn letter(code: u16) -> Option<Key> {
    LETTER_ROWS.iter().find_map(|(first, row)| {
        let index = code.checked_sub(first.0)? as usize;
        row.get(index).map(|c| Key::Char(*c))
    })
}

/// Key transition of one evdev event: `Some(true)` down, `Some(false)` up. Autorepeat
/// (value 2) is dropped because the core tracks held keys itself.
pub fn transition(event_type: u16, value: i32) -> Option<bool> {
    if event_type != EV_KEY {
        return None;
    }
    match value {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

pub fn classify(event_type: u16, code: u16, value: i32) -> Option<(Key, bool)> {
    Some((from_evdev(code)?, transition(event_type, value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: u16 = KeyCode::KEY_A.0;

    #[test]
    fn modifiers_map_left_and_right() {
        assert_eq!(from_evdev(KeyCode::KEY_RIGHTMETA.0), Some(Key::Super));
        assert_eq!(from_evdev(KeyCode::KEY_LEFTCTRL.0), Some(Key::Ctrl));
        assert_eq!(from_evdev(KeyCode::KEY_RIGHTCTRL.0), Some(Key::Ctrl));
        assert_eq!(from_evdev(KeyCode::KEY_LEFTALT.0), Some(Key::Alt));
        assert_eq!(from_evdev(KeyCode::KEY_RIGHTALT.0), Some(Key::Alt));
        assert_eq!(from_evdev(KeyCode::KEY_LEFTSHIFT.0), Some(Key::Shift));
        assert_eq!(from_evdev(KeyCode::KEY_RIGHTSHIFT.0), Some(Key::Shift));
        assert_eq!(from_evdev(KeyCode::KEY_LEFTMETA.0), Some(Key::Super));
    }

    #[test]
    fn function_keys_up_to_f24() {
        assert_eq!(from_evdev(KeyCode::KEY_F24.0), Some(Key::F(24)));
        let all = [
            KeyCode::KEY_F1, KeyCode::KEY_F2, KeyCode::KEY_F3, KeyCode::KEY_F4, KeyCode::KEY_F5,
            KeyCode::KEY_F6, KeyCode::KEY_F7, KeyCode::KEY_F8, KeyCode::KEY_F9, KeyCode::KEY_F10,
            KeyCode::KEY_F11, KeyCode::KEY_F12, KeyCode::KEY_F13, KeyCode::KEY_F14, KeyCode::KEY_F15,
            KeyCode::KEY_F16, KeyCode::KEY_F17, KeyCode::KEY_F18, KeyCode::KEY_F19, KeyCode::KEY_F20,
            KeyCode::KEY_F21, KeyCode::KEY_F22, KeyCode::KEY_F23, KeyCode::KEY_F24,
        ];
        for (i, code) in all.iter().enumerate() {
            assert_eq!(from_evdev(code.0), Some(Key::F(i as u8 + 1)));
        }
    }

    #[test]
    fn letters_digits_and_named_keys() {
        let letters = [
            (KeyCode::KEY_A, b'A'), (KeyCode::KEY_B, b'B'), (KeyCode::KEY_L, b'L'),
            (KeyCode::KEY_M, b'M'), (KeyCode::KEY_P, b'P'), (KeyCode::KEY_Q, b'Q'),
            (KeyCode::KEY_Z, b'Z'),
        ];
        for (code, c) in letters {
            assert_eq!(from_evdev(code.0), Some(Key::Char(c)));
        }
        assert_eq!(from_evdev(KeyCode::KEY_0.0), Some(Key::Char(b'0')));
        assert_eq!(from_evdev(KeyCode::KEY_1.0), Some(Key::Char(b'1')));
        assert_eq!(from_evdev(KeyCode::KEY_9.0), Some(Key::Char(b'9')));
        assert_eq!(from_evdev(KeyCode::KEY_SPACE.0), Some(Key::Space));
        assert_eq!(from_evdev(KeyCode::KEY_ENTER.0), Some(Key::Enter));
        assert_eq!(from_evdev(KeyCode::KEY_TAB.0), Some(Key::Tab));
        assert_eq!(from_evdev(KeyCode::KEY_BACKSPACE.0), Some(Key::Backspace));
        assert_eq!(from_evdev(KeyCode::KEY_ESC.0), None);
        assert_eq!(from_evdev(KeyCode::KEY_MINUS.0), None);
        assert_eq!(from_evdev(KeyCode::KEY_SEMICOLON.0), None);
    }

    #[test]
    fn repeat_events_are_ignored() {
        assert_eq!(classify(EV_KEY, KEY_A, 2), None);
        assert_eq!(classify(EV_KEY, KEY_A, 1), Some((Key::Char(b'A'), true)));
        assert_eq!(classify(EV_KEY, KEY_A, 0), Some((Key::Char(b'A'), false)));
        assert_eq!(classify(EventType::SYNCHRONIZATION.0, 0, 0), None);
    }
}
