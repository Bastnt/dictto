use std::collections::BTreeSet;

/// Physical key identity independent of the platform's key codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    Ctrl, Alt, Shift, Super, Space, Enter, Tab, Backspace,
    /// Upper-case ASCII letter or digit.
    Char(u8),
    /// Function key, 1..=24.
    F(u8),
}

impl Key {
    fn is_modifier(self) -> bool {
        matches!(self, Key::Ctrl | Key::Alt | Key::Shift | Key::Super)
    }

    /// Test-only source id; production callers pass real platform key codes.
    #[cfg(test)]
    fn id(self) -> u16 {
        match self {
            Key::Ctrl => 0, Key::Alt => 1, Key::Shift => 2, Key::Super => 3,
            Key::Space => 4, Key::Enter => 5, Key::Tab => 6, Key::Backspace => 7,
            Key::Char(c) => 0x100 + c as u16,
            Key::F(n) => 0x200 + n as u16,
        }
    }

    fn name(self) -> String {
        match self {
            Key::Ctrl => "Ctrl".into(), Key::Alt => "Alt".into(), Key::Shift => "Shift".into(),
            Key::Super => "Win".into(), Key::Space => "Space".into(), Key::Enter => "Enter".into(),
            Key::Tab => "Tab".into(), Key::Backspace => "Backspace".into(),
            Key::Char(c) => (c as char).to_string(),
            Key::F(n) => format!("F{n}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Chord { keys: BTreeSet<Key> }

impl Chord {
    pub fn overlaps(&self, other: &Self) -> bool {
        self.keys.is_subset(&other.keys) || other.keys.is_subset(&self.keys)
    }

    /// Canonical text; `parse(display())` round-trips and matches stored settings.
    pub fn display(&self) -> String {
        self.keys.iter().map(|k| k.name()).collect::<Vec<_>>().join("+")
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        let mut keys = BTreeSet::new();
        for part in value.split('+') {
            let token = part.trim().to_ascii_uppercase();
            let key = match token.as_str() {
                "CTRL" => Key::Ctrl, "ALT" => Key::Alt, "SHIFT" => Key::Shift,
                "WIN" | "SUPER" => Key::Super,
                "SPACE" => Key::Space, "ENTER" => Key::Enter, "TAB" => Key::Tab, "BACKSPACE" => Key::Backspace,
                _ if token.len() == 1 && token.as_bytes()[0].is_ascii_alphanumeric() => Key::Char(token.as_bytes()[0]),
                _ if token.starts_with('F') => {
                    let n: u16 = token[1..].parse().map_err(|_| "Unsupported shortcut key")?;
                    if !(1..=24).contains(&n) { return Err("Use F1 through F24".into()); }
                    Key::F(n as u8)
                }
                _ => return Err(format!("Unsupported shortcut key: {part}")),
            };
            if !keys.insert(key) { return Err("Duplicate shortcut key".into()); }
        }
        let modifiers = keys.iter().filter(|k| k.is_modifier()).count();
        if keys.is_empty() || keys.len() - modifiers > 1 ||
            (modifiers == 0 && !keys.iter().all(|k| matches!(k, Key::F(_)))) ||
            (keys.len() == modifiers && modifiers < 2) {
            return Err("Use modifiers with one key, two modifiers, or a function key".into());
        }
        Ok(Self { keys })
    }
}

pub struct Tracker {
    pub chord: Chord,
    /// Physical source -> key, so two physical keys mapping to one `Key` (left and
    /// right Ctrl) are released independently.
    held: Vec<(u16, Key)>,
    active: bool,
    pub suspended: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Self { chord: Chord::parse("Ctrl+Win").unwrap(), held: Vec::new(), active: false, suspended: false }
    }
}

impl Tracker {
    pub fn reset(&mut self) -> Option<bool> {
        self.held.clear();
        let active = std::mem::replace(&mut self.active, false);
        active.then_some(false)
    }

    #[cfg(test)]
    fn event(&mut self, key: Key, down: bool) -> Option<bool> {
        self.event_from(key.id(), key, down)
    }

    /// `source` identifies the physical key (the platform key code) when several
    /// physical keys share one `Key`.
    pub fn event_from(&mut self, source: u16, key: Key, down: bool) -> Option<bool> {
        if self.suspended { return None; }
        self.held.retain(|(s, _)| *s != source);
        if down { self.held.push((source, key)); }
        let active = self.chord.keys.iter().all(|k| self.held.iter().any(|(_, h)| h == k));
        if active == self.active { None } else { self.active = active; Some(active) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const LCTRL: u16 = 0xa2;
    const RCTRL: u16 = 0xa3;

    #[test]
    fn hold_release_and_repeat() {
        let mut t = Tracker::default();
        t.chord = Chord::parse("Ctrl+Shift+D").unwrap();
        assert_eq!(t.event(Key::Ctrl, true), None);
        assert_eq!(t.event(Key::Shift, true), None);
        assert_eq!(t.event(Key::Char(b'D'), true), Some(true));
        assert_eq!(t.event(Key::Char(b'D'), true), None);
        assert_eq!(t.event(Key::Shift, false), Some(false));
    }
    #[test]
    fn original_three_modifier_dictation_chord_works_in_either_order() {
        for keys in [[Key::Ctrl, Key::Alt, Key::Super], [Key::Super, Key::Alt, Key::Ctrl]] {
            let mut tracker = Tracker::default();
            tracker.chord = Chord::parse("Ctrl+Alt+Win").unwrap();
            assert_eq!(tracker.event(keys[0], true), None);
            assert_eq!(tracker.event(keys[1], true), None);
            assert_eq!(tracker.event(keys[2], true), Some(true));
            assert_eq!(tracker.event(keys[2], true), None);
            assert_eq!(tracker.event(keys[0], false), Some(false));
        }
    }
    #[test]
    fn releasing_one_of_two_control_keys_keeps_chord() {
        let mut t = Tracker::default();
        t.event_from(LCTRL, Key::Ctrl, true); t.event_from(RCTRL, Key::Ctrl, true);
        assert_eq!(t.event(Key::Super, true), Some(true));
        assert_eq!(t.event_from(LCTRL, Key::Ctrl, false), None);
        assert_eq!(t.event_from(RCTRL, Key::Ctrl, false), Some(false));
    }
    #[test]
    fn invalid_shortcuts_and_capture_reset() {
        for value in ["", "Ctrl", "A", "Ctrl+A+B", "Ctrl+Ctrl", "F25"] { assert!(Chord::parse(value).is_err()); }
        assert!(Chord::parse("F8").is_ok());
        let mut t = Tracker::default();
        t.event(Key::Ctrl, true); t.event(Key::Super, true);
        assert_eq!(t.reset(), Some(false));
        t.suspended = true;
        assert_eq!(t.event(Key::Ctrl, true), None);
        t.suspended = false;
        assert_eq!(t.event(Key::Super, true), None);
    }
    #[test]
    fn error_messages_are_unchanged() {
        assert_eq!(Chord::parse("F25").unwrap_err(), "Use F1 through F24");
        assert_eq!(Chord::parse("F300").unwrap_err(), "Use F1 through F24");
        assert_eq!(Chord::parse("Ctrl+Ctrl").unwrap_err(), "Duplicate shortcut key");
        assert_eq!(Chord::parse("Ctrl+Bar").unwrap_err(), "Unsupported shortcut key: Bar");
        assert_eq!(Chord::parse("Ctrl").unwrap_err(), "Use modifiers with one key, two modifiers, or a function key");
    }
    #[test] fn win_and_super_are_the_same_key() { assert!(Chord::parse("Ctrl+Super").unwrap().overlaps(&Chord::parse("Ctrl+Win").unwrap())); }
    #[test] fn stored_strings_round_trip() {
        assert_eq!(Chord::parse("CTRL+SHIFT+F8").unwrap().display(), "Ctrl+Shift+F8");
        assert_eq!(Chord::parse("Win+Ctrl+q").unwrap().display(), "Ctrl+Win+Q");
    }
    #[test] fn reconfigure_while_held_releases() {
        let mut t = Tracker { chord: Chord::parse("Ctrl+Win").unwrap(), ..Default::default() };
        t.event(Key::Ctrl, true); assert_eq!(t.event(Key::Super, true), Some(true));
        assert_eq!(t.reset(), Some(false));
    }
}
