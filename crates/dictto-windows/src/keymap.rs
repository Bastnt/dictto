use dictto_core::keys::Key;

/// Left and right modifier variants collapse to one `Key`; unmapped codes are `None`.
pub fn from_vk(vk: u16) -> Option<Key> {
    Some(match vk {
        0x11 | 0xa2 | 0xa3 => Key::Ctrl,
        0x12 | 0xa4 | 0xa5 => Key::Alt,
        0x10 | 0xa0 | 0xa1 => Key::Shift,
        0x5b | 0x5c => Key::Super,
        0x20 => Key::Space,
        0x0d => Key::Enter,
        0x09 => Key::Tab,
        0x08 => Key::Backspace,
        0x30..=0x39 | 0x41..=0x5a => Key::Char(vk as u8),
        0x70..=0x87 => Key::F((vk - 0x6f) as u8),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_and_right_modifiers_map_to_one_key() {
        assert_eq!(from_vk(0xa2), Some(Key::Ctrl));
        assert_eq!(from_vk(0xa3), Some(Key::Ctrl));
        assert_eq!(from_vk(0x5c), Some(Key::Super));
        assert_eq!(from_vk(0xa1), Some(Key::Shift));
        assert_eq!(from_vk(0xa5), Some(Key::Alt));
    }

    #[test]
    fn letters_digits_and_function_keys() {
        assert_eq!(from_vk(0x44), Some(Key::Char(b'D')));
        assert_eq!(from_vk(0x35), Some(Key::Char(b'5')));
        assert_eq!(from_vk(0x70), Some(Key::F(1)));
        assert_eq!(from_vk(0x87), Some(Key::F(24)));
        assert_eq!(from_vk(0x88), None);
        assert_eq!(from_vk(0x1b), None);
    }

    #[test]
    fn modifier_keys_are_exactly_the_former_modifier_virtual_keys() {
        // Speech shortcuts never swallow modifiers, so every modifier VK must map to a modifier key.
        for vk in 0..=0xffu16 {
            let modifier = matches!(from_vk(vk), Some(Key::Ctrl | Key::Alt | Key::Shift | Key::Super));
            assert_eq!(modifier, matches!(vk, 0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5), "{vk:#x}");
        }
    }
}
