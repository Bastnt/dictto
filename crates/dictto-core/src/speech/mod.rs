//! Speech synthesis shortcuts: read the selection aloud, repeat dictated speech, and
//! record the clone sample. OS services are reached through `SpeechPlatform`.
use crate::audio::DeviceSelection;
use crate::host::{AppHandle, Manager};
use crate::keys::{Chord, Key, Tracker};
use crate::transcription::{openai_api::OpenAIWhisper, TranscriptionConfig, TranscriptionEngine};
use crate::CoreState;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub mod api;
pub mod cache;
mod render;
pub mod route;
pub(crate) mod session;
pub mod virtual_mic;

pub use render::Player;

/// Returns the selected text, else the clipboard text; never writes the clipboard.
pub trait SelectionReader: Send + Sync {
    fn read(&self) -> Result<String, String>;
}

/// Routes one read to a virtual microphone that other applications capture.
pub trait VirtualMic: Send + Sync {
    /// Fails with a user-facing reason before any API call is made.
    fn check_available(&self) -> Result<(), String>;
    /// The route is restored when the returned player is dropped.
    fn player(&self) -> Box<dyn Player + Send>;
}

pub struct SpeechPlatform {
    /// Plays to the user's speakers.
    pub player: Arc<dyn Player + Send>,
    pub virtual_mic: Arc<dyn VirtualMic>,
    pub selection: Arc<dyn SelectionReader>,
    pub cache_files: cache::CacheFiles,
}

/// Speech state registered on the host next to `CoreState` by apps with speech.
/// Derefs to `CoreState` so speech code reads shared fields through one handle; the
/// `speech` field is named apart from `CoreState::platform` so the deref never hides it.
pub struct SpeechState {
    core: Arc<CoreState>,
    pub speech: SpeechPlatform,
    cache: Mutex<Option<cache::Cache>>,
    pub audio_dir: PathBuf,
    pub sample_path: PathBuf,
}

impl SpeechState {
    pub fn new(core: Arc<CoreState>, data_dir: &Path, platform: SpeechPlatform) -> Result<Self, String> {
        let audio_dir = std::path::absolute(data_dir.join("Speech Audio")).map_err(|e| e.to_string())?;
        let cache = cache::Cache::new(audio_dir.clone(), platform.cache_files)
            .map_err(|e| log::warn!("Could not open speech audio folder: {e}"))
            .ok();
        Ok(Self {
            core,
            speech: platform,
            cache: Mutex::new(cache),
            audio_dir,
            sample_path: data_dir.join("voice-sample.wav"),
        })
    }
}

impl std::ops::Deref for SpeechState {
    type Target = CoreState;
    fn deref(&self) -> &CoreState {
        &self.core
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SampleMic,
    SamplePlayback,
    Repeat,
    ReadClone,
    ReadVoice,
}

/// Setting key of each speech shortcut.
pub const BINDINGS: [(&str, Action); 5] = [
    ("tts_hotkey_sample_mic", Action::SampleMic),
    ("tts_hotkey_sample_loopback", Action::SamplePlayback),
    ("tts_hotkey_speech_to_speech", Action::Repeat),
    ("tts_hotkey_read_clone", Action::ReadClone),
    ("tts_hotkey_read_voice", Action::ReadVoice),
];

#[derive(Default)]
struct Shortcuts {
    bindings: Vec<(Action, Tracker)>,
    suppressed: BTreeSet<u16>,
    suspended: bool,
}

fn shortcuts() -> &'static Mutex<Shortcuts> {
    static SHORTCUTS: OnceLock<Mutex<Shortcuts>> = OnceLock::new();
    SHORTCUTS.get_or_init(Default::default)
}

pub fn configure(mut bindings: Vec<(Action, Tracker)>) {
    let mut current = shortcuts().lock().unwrap();
    for (_, tracker) in &mut bindings {
        tracker.suspended = current.suspended;
    }
    current.bindings = bindings;
}

/// Validates against the dictation shortcut too; `strict` rejects instead of skipping bad entries.
pub fn parse_bindings(
    settings: &HashMap<String, String>,
    strict: bool,
) -> Result<Vec<(Action, Tracker)>, String> {
    let dictation = settings
        .get("hotkey")
        .and_then(|s| Chord::parse(s).ok())
        .unwrap_or_else(|| Chord::parse("Ctrl+Win").unwrap());
    let mut chords = vec![dictation];
    let mut result = Vec::new();
    for (key, action) in BINDINGS {
        let Some(binding) = settings.get(key).filter(|v| !v.is_empty()) else {
            continue;
        };
        let chord = Chord::parse(binding).and_then(|chord| {
            if chords.iter().any(|other| chord.overlaps(other)) {
                Err(
                    "Shortcuts must be distinct and cannot contain another configured shortcut."
                        .to_string(),
                )
            } else {
                Ok(chord)
            }
        });
        let chord = match chord {
            Ok(c) => c,
            Err(e) if strict => return Err(e),
            Err(e) => {
                log::warn!("Skipping {key}: {e}");
                continue;
            }
        };
        chords.push(chord.clone());
        let mut tracker = Tracker::default();
        tracker.chord = chord;
        result.push((action, tracker));
    }
    Ok(result)
}

impl Shortcuts {
    fn event(&mut self, source: u16, key: Option<Key>, down: bool) -> (Option<Action>, bool) {
        let mut hit = None;
        for (action, tracker) in &mut self.bindings {
            let transition = key.and_then(|key| tracker.event_from(source, key, down));
            if transition == Some(true) && hit.is_none() {
                hit = Some(*action);
            }
        }
        // Keep the function key and its repeats out of the focused application's accelerators.
        let modifier = matches!(key, Some(Key::Ctrl | Key::Alt | Key::Shift | Key::Super));
        if hit.is_some() && !modifier {
            self.suppressed.insert(source);
        }
        let suppress = if down {
            self.suppressed.contains(&source)
        } else {
            self.suppressed.remove(&source)
        };
        (hit, suppress)
    }
}

/// Returns the action whose chord this event completes and whether to swallow the event.
pub(crate) fn key_event(source: u16, key: Option<Key>, down: bool) -> (Option<Action>, bool) {
    shortcuts().lock().unwrap().event(source, key, down)
}

pub(crate) fn suspend(active: bool) {
    let mut current = shortcuts().lock().unwrap();
    current.suspended = active;
    for (_, tracker) in &mut current.bindings {
        tracker.reset();
        tracker.suspended = active;
    }
}

impl Action {
    pub(crate) fn records(self) -> bool {
        matches!(self, Self::SampleMic | Self::SamplePlayback | Self::Repeat)
    }
    fn sample(self) -> bool {
        matches!(self, Self::SampleMic | Self::SamplePlayback)
    }
    pub(crate) fn start(self, app: &AppHandle) -> Result<Duration, String> {
        let state = app.state::<SpeechState>();
        if self == Self::Repeat && !state.sample_path.is_file() {
            return Err("Record a clone sample first.".into());
        }
        let settings = state
            .db
            .lock()
            .unwrap()
            .get_all_settings()
            .map_err(|e| e.to_string())?;
        let device = match (self, settings.get("microphone_device").map(String::as_str)) {
            (Self::SamplePlayback, _) => DeviceSelection::PlaybackLoopback,
            (_, None | Some("auto-detect")) => {
                DeviceSelection::AutoDetect(state.device_monitor.get_device_id())
            }
            (_, Some(name)) => DeviceSelection::ByName(name.into()),
        };
        state
            .audio_recorder
            .lock()
            .unwrap()
            .start(device)
            .map_err(|e| e.to_string())?;
        let secs = settings
            .get("recording_limit_seconds")
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        Ok(Duration::from_secs(if self.sample() {
            secs.min(29)
        } else {
            secs
        }))
    }

    pub(crate) async fn run(self, app: AppHandle, audio: Option<Vec<u8>>) -> Result<(), String> {
        let state = app.state::<SpeechState>();
        if self.sample() {
            let wav = audio.ok_or("No recorded sample")?;
            return replace_sample(&state.sample_path, &wav);
        }
        let sample = if self != Self::ReadVoice {
            Some(std::fs::read(&state.sample_path).map_err(|_| "Record a clone sample first.")?)
        } else {
            None
        };
        let settings = state
            .db
            .lock()
            .unwrap()
            .get_all_settings()
            .map_err(|e| e.to_string())?;
        let get = |key: &str| settings.get(key).map(String::as_str).unwrap_or("");
        let use_virtual_mic = get("tts_virtual_mic") == "true";
        if use_virtual_mic {
            state.speech.virtual_mic.check_available()?;
        }
        let key = crate::db::keystore::get_api_key()
            .map_err(|_| "Set your fleet API key in Settings.")?;
        let base = get("api_base_url");
        let languages: Vec<String> = serde_json::from_str(get("languages")).unwrap_or_default();
        let config = TranscriptionConfig::from_languages(&languages);
        let text = if self == Self::Repeat {
            let whisper = OpenAIWhisper::with_client(
                key.clone(),
                state.http_client.clone(),
                base.into(),
                get("transcription_model").into(),
            );
            whisper
                .transcribe(&audio.ok_or("No recorded speech")?, &config)
                .await
                .map_err(|e| crate::transcription::friendly_error(&e.to_string()))?
                .text
        } else {
            let selection = state.speech.selection.clone();
            tokio::task::spawn_blocking(move || selection.read())
                .await
                .map_err(|_| "Could not read selected text")??
        };
        if text.trim().is_empty() {
            return Err("No text to speak.".into());
        }
        let microphone = use_virtual_mic.then(|| state.speech.virtual_mic.player());
        let player: &dyn Player = match &microphone {
            Some(microphone) => &**microphone,
            None => &*state.speech.player,
        };
        render::speak(
            &state,
            &settings,
            &key,
            &text,
            config.language.as_deref().unwrap_or(""),
            sample.as_deref(),
            player,
        )
        .await
    }
}

fn replace_sample(path: &Path, wav: &[u8]) -> Result<(), String> {
    let reader = hound::WavReader::new(wav).map_err(|_| "Invalid sample recording")?;
    let rate = reader.spec().sample_rate;
    if reader.duration() < rate * 3 / 10 || reader.duration() > rate * 30 {
        return Err("Record a sample between 0.3 and 30 seconds.".into());
    }
    let temp = path.with_extension("pending");
    let result = std::fs::write(&temp, wav).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|e| format!("Could not save clone sample: {e}"))
}

fn speech_chunks(mut text: &str) -> impl Iterator<Item = &str> {
    let mut opening = true;
    std::iter::from_fn(move || {
        while !text.is_empty() {
            if opening && text.chars().nth(450).is_some() {
                opening = false;
                let mut boundary = None;
                for (n, (i, ch)) in text.char_indices().take(350).enumerate() {
                    if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n')
                        && (matches!(ch, '。' | '！' | '？' | '\n')
                            || text[i + ch.len_utf8()..].starts_with(char::is_whitespace))
                    {
                        boundary = Some(i + ch.len_utf8());
                        if n >= 150 {
                            break;
                        }
                    }
                }
                if let Some(end) = boundary {
                    let (chunk, rest) = text.split_at(end);
                    text = rest;
                    if !chunk.trim().is_empty() {
                        return Some(chunk);
                    }
                }
            }
            let limit = text.char_indices().nth(4096).map_or(text.len(), |(i, _)| i);
            let candidate = &text[..limit];
            let end = if limit == text.len() {
                limit
            } else {
                candidate
                    .rmatch_indices(['.', '!', '?', '。', '！', '？', '\n'])
                    .find(|(i, _)| *i >= limit / 2)
                    .or_else(|| {
                        candidate
                            .rmatch_indices(char::is_whitespace)
                            .find(|(i, _)| *i >= limit / 2)
                    })
                    .map_or(limit, |(i, m)| i + m.len())
            };
            let (chunk, rest) = text.split_at(end);
            text = rest;
            if !chunk.trim().is_empty() {
                return Some(chunk);
            }
        }
        None
    })
}

/// Helper for `SelectionReader` implementations; the clipboard is read only without a selection.
pub fn preferred_text(
    selected: Option<String>,
    clipboard: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    selected
        .filter(|s| !s.trim().is_empty())
        .map(Ok)
        .unwrap_or_else(clipboard)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Unused;
    #[async_trait::async_trait]
    impl Player for Unused {
        async fn chunk(&self, _: Vec<u8>) -> Result<(), String> {
            unreachable!()
        }
        async fn file(&self, _: &Path) -> Result<(), String> {
            unreachable!()
        }
    }
    impl VirtualMic for Unused {
        fn check_available(&self) -> Result<(), String> {
            unreachable!()
        }
        fn player(&self) -> Box<dyn Player + Send> {
            unreachable!()
        }
    }
    impl SelectionReader for Unused {
        fn read(&self) -> Result<String, String> {
            unreachable!()
        }
    }
    pub(super) fn platform() -> SpeechPlatform {
        SpeechPlatform {
            player: Arc::new(Unused),
            virtual_mic: Arc::new(Unused),
            selection: Arc::new(Unused),
            cache_files: cache::CacheFiles::default(),
        }
    }

    #[test]
    fn split_long_text_at_unicode_boundaries_without_losing_content() {
        let text = format!("{} {}. {}", "é".repeat(3900), "end", "語".repeat(8300));
        let chunks: Vec<_> = speech_chunks(&text).collect();
        assert!(chunks.iter().all(|s| s.chars().count() <= 4096));
        assert!(chunks[0].ends_with('.'));
        assert_eq!(chunks.concat(), text);
        assert!(speech_chunks(&" ".repeat(9000)).next().is_none());
    }
    #[test]
    fn selection_wins_and_clipboard_fallback_is_lazy() {
        assert_eq!(
            preferred_text(Some("selected".into()), || panic!("read clipboard")).unwrap(),
            "selected"
        );
        for selected in [None, Some("".into()), Some("  ".into())] {
            assert_eq!(
                preferred_text(selected, || Ok("clipboard".into())).unwrap(),
                "clipboard"
            );
        }
    }
    #[test]
    fn failed_sample_preserves_previous_file() {
        let path =
            std::env::temp_dir().join(format!("dictto-sample-test-{}.wav", std::process::id()));
        let mut output = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut output, spec).unwrap();
        for _ in 0..5000 {
            writer.write_sample(100i16).unwrap();
        }
        writer.finalize().unwrap();
        replace_sample(&path, output.get_ref()).unwrap();
        let mut short = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut short, spec).unwrap();
        writer.write_sample(100i16).unwrap();
        writer.finalize().unwrap();
        assert!(replace_sample(&path, short.get_ref()).is_err());
        assert!(replace_sample(&path, &[]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), *output.get_ref());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn read_hotkey_keeps_the_function_key_out_of_the_focused_app() {
        let settings = HashMap::from([("tts_hotkey_read_clone".into(), "Ctrl+Shift+F9".into())]);
        let mut chords = Shortcuts {
            bindings: parse_bindings(&settings, true).unwrap(),
            ..Default::default()
        };
        let (ctrl, shift, f9) = (Some(Key::Ctrl), Some(Key::Shift), Some(Key::F(9)));
        assert_eq!(chords.event(0xa2, ctrl, true), (None, false));
        assert_eq!(chords.event(0xa0, shift, true), (None, false));
        assert_eq!(chords.event(0x78, f9, true), (Some(Action::ReadClone), true));
        assert_eq!(chords.event(0x78, f9, true), (None, true));
        assert_eq!(chords.event(0xa2, ctrl, false), (None, false));
        assert_eq!(chords.event(0x78, f9, false), (None, true));
        assert_eq!(chords.event(0x78, f9, true), (None, false));
    }

    #[test]
    fn modifier_completing_a_chord_and_unmapped_keys_reach_the_focused_app() {
        let settings = HashMap::from([
            ("hotkey".into(), "F8".into()),
            ("tts_hotkey_read_voice".into(), "Ctrl+Alt".into()),
        ]);
        let mut chords = Shortcuts {
            bindings: parse_bindings(&settings, true).unwrap(),
            ..Default::default()
        };
        assert_eq!(chords.event(0xa2, Some(Key::Ctrl), true), (None, false));
        assert_eq!(chords.event(0x1b, None, true), (None, false));
        assert_eq!(chords.event(0xa4, Some(Key::Alt), true), (Some(Action::ReadVoice), false));
        assert_eq!(chords.event(0xa4, Some(Key::Alt), false), (None, false));
        assert_eq!(chords.event(0x1b, None, false), (None, false));
    }

    #[test]
    fn suspended_shortcuts_neither_fire_nor_swallow() {
        let settings = HashMap::from([("tts_hotkey_read_clone".into(), "F9".into())]);
        let mut chords = Shortcuts {
            bindings: parse_bindings(&settings, true).unwrap(),
            ..Default::default()
        };
        for (_, tracker) in &mut chords.bindings {
            tracker.suspended = true;
        }
        assert_eq!(chords.event(0x78, Some(Key::F(9)), true), (None, false));
    }

    #[test]
    fn bindings_reject_collisions_with_dictation_or_other_speech_actions() {
        let mut settings = HashMap::new();
        settings.insert("tts_hotkey_read_clone".into(), "Win+Ctrl+Q".into());
        assert!(parse_bindings(&settings, true).is_err());
        settings.insert("tts_hotkey_read_clone".into(), "F9".into());
        settings.insert("tts_hotkey_read_voice".into(), "Ctrl+F9".into());
        assert!(parse_bindings(&settings, true).is_err());
        settings.insert("tts_hotkey_read_voice".into(), "F10".into());
        assert_eq!(parse_bindings(&settings, true).unwrap().len(), 2);
    }
}
