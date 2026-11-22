use super::*;

pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("hotkey", "Dictate (hold; double-press to keep recording)"),
    ("tts_hotkey_sample_mic", "Record a voice sample from the microphone"),
    ("tts_hotkey_sample_loopback", "Record a voice sample from Windows playback"),
    ("tts_hotkey_speech_to_speech", "Repeat my speech in the cloned voice"),
    ("tts_hotkey_read_clone", "Read text in the cloned voice"),
    ("tts_hotkey_read_voice", "Read text in the engine voice"),
];
const FIELD: usize = 630;
const CLEAR: usize = 640;
const DEFAULT_HOTKEY: &str = "Ctrl+Win";

pub(super) fn is_field(id: usize) -> bool {
    (FIELD..FIELD + SHORTCUTS.len()).contains(&id)
}

fn held(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

/// The chord as the settings spell it, from the keys held now plus `vk`.
fn chord(vk: VIRTUAL_KEY) -> Result<String, &'static str> {
    let mut parts: Vec<String> = [
        (held(VK_CONTROL), "Ctrl"),
        (held(VK_MENU), "Alt"),
        (held(VK_SHIFT), "Shift"),
        (held(VK_LWIN) || held(VK_RWIN), "Win"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_string())
    .collect();
    let key = match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_MENU | VK_LMENU | VK_RMENU | VK_SHIFT
        | VK_LSHIFT | VK_RSHIFT | VK_LWIN | VK_RWIN => None,
        VK_SPACE => Some("Space".to_string()),
        VK_RETURN => Some("Enter".into()),
        VK_TAB => Some("Tab".into()),
        VK_BACK => Some("Backspace".into()),
        _ if (VK_F1.0..=VK_F24.0).contains(&vk.0) => Some(format!("F{}", vk.0 - VK_F1.0 + 1)),
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk.0) || (b'0' as u16..=b'9' as u16).contains(&vk.0) => {
            Some(char::from(vk.0 as u8).to_string())
        }
        _ => return Err("Use a letter, digit, function key, Space, Enter, Tab or Backspace."),
    };
    parts.extend(key);
    Ok(parts.join("+"))
}

impl App {
    pub(super) unsafe fn render_shortcuts(&mut self) {
        self.hint(
            "Click a field, then press the keys together. Escape cancels.",
            [20, 64, 614, 22],
        );
        for (i, (key, label)) in SHORTCUTS.iter().enumerate() {
            let y = 100 + 44 * i as i32;
            self.label(label, [20, y + 4, 310, 22]);
            let value = self.setting(key).to_string();
            let field = self.add(
                FIELD + i,
                w!("EDIT"),
                &value,
                WS_TABSTOP | WS_BORDER | WINDOW_STYLE((ES_AUTOHSCROLL | ES_READONLY) as u32),
                [340, y, 200, 28],
            );
            self.cue(field, "Not set");
            self.button(CLEAR + i, if i == 0 { "Reset" } else { "Clear" }, [550, y, 84, 28]);
        }
        self.hint(
            "Read text uses the selection in the focused app, or the clipboard when nothing is selected. Clone actions use your last voice sample.",
            [20, 372, 614, 40],
        );
    }

    unsafe fn restore_shortcut(&mut self, index: usize) {
        self.capture = None;
        self.label_text(FIELD + index, &self.setting(SHORTCUTS[index].0).to_string());
    }

    pub(super) unsafe fn shortcut_command(&mut self, id: usize, notification: u32) -> bool {
        if is_field(id) {
            match notification {
                EN_SETFOCUS => self.runtime.set_hotkey_capture(true),
                EN_KILLFOCUS => {
                    self.runtime.set_hotkey_capture(false);
                    if !self.control(id).0.is_null() {
                        self.restore_shortcut(id - FIELD);
                    }
                }
                _ => {}
            }
            return true;
        }
        if (CLEAR..CLEAR + SHORTCUTS.len()).contains(&id) && notification == BN_CLICKED {
            let index = id - CLEAR;
            let value = if index == 0 { DEFAULT_HOTKEY } else { "" };
            self.persist(SHORTCUTS[index].0, value);
            self.restore_shortcut(index);
            return true;
        }
        false
    }

    /// Key messages aimed at a shortcut field become the chord they spell instead of text.
    /// Returns whether the message was consumed.
    pub(super) unsafe fn shortcut_key(&mut self, message: &MSG) -> bool {
        let Some(index) = (0..SHORTCUTS.len()).find(|i| self.control(FIELD + i) == message.hwnd) else {
            return false;
        };
        let vk = VIRTUAL_KEY(message.wParam.0 as u16);
        match message.message {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let plain_tab = vk == VK_TAB
                    && ![VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN].into_iter().any(held);
                if plain_tab {
                    // Tab and Shift+Tab keep moving focus.
                    return false;
                }
                if vk == VK_ESCAPE {
                    self.restore_shortcut(index);
                    let _ = SetFocus(Some(self.control(CLEAR + index)));
                    return true;
                }
                match chord(vk) {
                    Ok(text) => {
                        self.label_text(FIELD + index, &text);
                        self.capture = dictto_core::keys::Chord::parse(&text).is_ok().then_some(text);
                    }
                    Err(reason) => {
                        self.capture = None;
                        self.status = reason.into();
                        self.label_text(STATUS, reason);
                    }
                }
                true
            }
            WM_KEYUP | WM_SYSKEYUP => {
                if let Some(text) = self.capture.take() {
                    if self.persist(SHORTCUTS[index].0, &text) {
                        let _ = SetFocus(Some(self.control(CLEAR + index)));
                    } else {
                        self.restore_shortcut(index);
                    }
                    self.label_text(STATUS, &self.status);
                }
                true
            }
            _ => false,
        }
    }
}
