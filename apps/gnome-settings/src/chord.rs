//! Turns the keys pressed in a shortcut field into the stored chord text. The daemon
//! suspends its own shortcuts meanwhile (SetHotkeyCapture), so the field sees every key.
use dictto_core::keys::{Chord, Key};

#[derive(Debug, PartialEq)]
pub(crate) enum Recorded {
    /// Nothing to show yet: modifiers are still held.
    Pending,
    Chord(String),
    /// Backspace alone empties the field, which disables a speech shortcut.
    Clear,
    Invalid(String),
}

#[derive(Default)]
pub(crate) struct ChordRecorder {
    held: Vec<Key>,
    /// Every modifier held since all keys were last up, so `Ctrl+Win` is recorded on release.
    modifiers: Vec<Key>,
    /// A non-modifier key already produced the result for this press sequence.
    done: bool,
}

fn is_modifier(key: Key) -> bool {
    matches!(key, Key::Ctrl | Key::Alt | Key::Shift | Key::Super)
}

fn token(key: Key) -> String {
    match key {
        Key::Ctrl => "Ctrl".into(),
        Key::Alt => "Alt".into(),
        Key::Shift => "Shift".into(),
        Key::Super => "Win".into(),
        Key::Space => "Space".into(),
        Key::Enter => "Enter".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "Backspace".into(),
        Key::Char(c) => (c as char).to_string(),
        Key::F(n) => format!("F{n}"),
    }
}

fn chord<'a>(keys: impl Iterator<Item = &'a Key>) -> Recorded {
    let text = keys.map(|k| token(*k)).collect::<Vec<_>>().join("+");
    match Chord::parse(&text) {
        Ok(chord) => Recorded::Chord(chord.display()),
        Err(error) => Recorded::Invalid(error),
    }
}

impl ChordRecorder {
    /// True while a modifier is down, so the caller lets a bare Tab move focus.
    pub fn holds_modifier(&self) -> bool {
        self.held.iter().any(|k| is_modifier(*k))
    }

    pub fn press(&mut self, key: Key) -> Recorded {
        if !self.held.contains(&key) {
            self.held.push(key);
        }
        if is_modifier(key) {
            if !self.modifiers.contains(&key) {
                self.modifiers.push(key);
            }
            return Recorded::Pending;
        }
        self.done = true;
        let modifiers: Vec<Key> = self.held.iter().copied().filter(|k| is_modifier(*k)).collect();
        if modifiers.is_empty() && key == Key::Backspace {
            return Recorded::Clear;
        }
        chord(modifiers.iter().chain(std::iter::once(&key)))
    }

    pub fn release(&mut self, key: Key) -> Recorded {
        self.held.retain(|k| *k != key);
        if !self.held.is_empty() {
            return Recorded::Pending;
        }
        let modifiers = std::mem::take(&mut self.modifiers);
        if std::mem::take(&mut self.done) || modifiers.len() < 2 {
            return Recorded::Pending;
        }
        chord(modifiers.iter())
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_and_a_key_record_on_press() {
        let mut recorder = ChordRecorder::default();
        assert_eq!(recorder.press(Key::Ctrl), Recorded::Pending);
        assert_eq!(recorder.press(Key::Shift), Recorded::Pending);
        assert_eq!(recorder.press(Key::F(8)), Recorded::Chord("Ctrl+Shift+F8".into()));
        assert_eq!(recorder.release(Key::F(8)), Recorded::Pending);
        assert_eq!(recorder.release(Key::Shift), Recorded::Pending);
        assert_eq!(recorder.release(Key::Ctrl), Recorded::Pending);
    }

    #[test]
    fn modifier_only_chords_record_on_release() {
        let mut recorder = ChordRecorder::default();
        recorder.press(Key::Super);
        recorder.press(Key::Ctrl);
        assert_eq!(recorder.release(Key::Super), Recorded::Pending);
        assert_eq!(recorder.release(Key::Ctrl), Recorded::Chord("Ctrl+Win".into()));
        // A lone modifier is not a shortcut and leaves the field alone.
        recorder.press(Key::Alt);
        assert_eq!(recorder.release(Key::Alt), Recorded::Pending);
    }

    #[test]
    fn backspace_clears_and_bad_chords_explain() {
        let mut recorder = ChordRecorder::default();
        assert_eq!(recorder.press(Key::Backspace), Recorded::Clear);
        recorder.release(Key::Backspace);
        assert_eq!(
            recorder.press(Key::Char(b'A')),
            Recorded::Invalid("Use modifiers with one key, two modifiers, or a function key".into())
        );
        recorder.release(Key::Char(b'A'));
        assert_eq!(recorder.press(Key::F(5)), Recorded::Chord("F5".into()));
        recorder.reset();
        recorder.press(Key::Ctrl);
        assert!(recorder.holds_modifier());
        assert_eq!(recorder.press(Key::Backspace), Recorded::Chord("Ctrl+Backspace".into()));
    }
}
