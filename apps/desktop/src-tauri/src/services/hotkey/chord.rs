use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct Chord { keys: BTreeSet<u16> }

impl Chord {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut keys = BTreeSet::new();
        for part in value.split('+') {
            let token = part.trim().to_ascii_uppercase();
            let vk = match token.as_str() {
                "CTRL" => 0x11, "ALT" => 0x12, "SHIFT" => 0x10, "WIN" => 0x5b,
                "SPACE" => 0x20, "ENTER" => 0x0d, "TAB" => 0x09, "BACKSPACE" => 0x08,
                _ if token.len() == 1 && token.as_bytes()[0].is_ascii_alphanumeric() => token.as_bytes()[0] as u16,
                _ if token.starts_with('F') => {
                    let n: u16 = token[1..].parse().map_err(|_| "Unsupported shortcut key")?;
                    if !(1..=24).contains(&n) { return Err("Use F1 through F24".into()); }
                    0x6f + n
                }
                _ => return Err(format!("Unsupported shortcut key: {part}")),
            };
            if !keys.insert(vk) { return Err("Duplicate shortcut key".into()); }
        }
        let modifiers = keys.iter().filter(|k| matches!(**k, 0x10 | 0x11 | 0x12 | 0x5b)).count();
        if keys.is_empty() || keys.len() - modifiers > 1 ||
            (modifiers == 0 && !keys.iter().all(|k| (0x70..=0x87).contains(k))) ||
            (keys.len() == modifiers && modifiers < 2) {
            return Err("Use modifiers with one key, two modifiers, or a function key".into());
        }
        Ok(Self { keys })
    }
}

pub struct Tracker {
    pub chord: Chord,
    held: BTreeSet<u16>,
    active: bool,
    pub suspended: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Self { chord: Chord::parse("Ctrl+Win").unwrap(), held: BTreeSet::new(), active: false, suspended: false }
    }
}

impl Tracker {
    pub fn reset(&mut self) -> Option<bool> {
        self.held.clear();
        let active = std::mem::replace(&mut self.active, false);
        active.then_some(false)
    }
    pub fn event(&mut self, vk: u16, down: bool) -> Option<bool> {
        if self.suspended { return None; }
        if down { self.held.insert(vk); } else { self.held.remove(&vk); }
        let normalized: BTreeSet<_> = self.held.iter().map(|k| match *k {
            0xa0 | 0xa1 => 0x10, 0xa2 | 0xa3 => 0x11, 0xa4 | 0xa5 => 0x12, 0x5c => 0x5b, other => other,
        }).collect();
        let active = self.chord.keys.is_subset(&normalized);
        if active == self.active { None } else { self.active = active; Some(active) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hold_release_and_repeat() {
        let mut t = Tracker::default();
        t.chord = Chord::parse("Ctrl+Shift+D").unwrap();
        assert_eq!(t.event(0xa3, true), None);
        assert_eq!(t.event(0xa0, true), None);
        assert_eq!(t.event(0x44, true), Some(true));
        assert_eq!(t.event(0x44, true), None);
        assert_eq!(t.event(0xa0, false), Some(false));
    }
    #[test]
    fn releasing_one_of_two_control_keys_keeps_chord() {
        let mut t = Tracker::default();
        t.event(0xa2, true); t.event(0xa3, true);
        assert_eq!(t.event(0x5b, true), Some(true));
        assert_eq!(t.event(0xa2, false), None);
        assert_eq!(t.event(0xa3, false), Some(false));
    }
    #[test]
    fn invalid_shortcuts_and_capture_reset() {
        for value in ["", "Ctrl", "A", "Ctrl+A+B", "Ctrl+Ctrl", "F25"] { assert!(Chord::parse(value).is_err()); }
        assert!(Chord::parse("F8").is_ok());
        let mut t = Tracker::default();
        t.event(0xa2, true); t.event(0x5b, true);
        assert_eq!(t.reset(), Some(false));
        t.suspended = true;
        assert_eq!(t.event(0xa2, true), None);
        t.suspended = false;
        assert_eq!(t.event(0x5b, true), None);
    }
}
