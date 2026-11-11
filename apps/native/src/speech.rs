use crate::host::{AppHandle, Manager};
use dictto_core::audio::DeviceSelection;
use dictto_core::keys::{Chord, Tracker};
use dictto_core::transcription::{
    openai_api::OpenAIWhisper, TranscriptionConfig, TranscriptionEngine,
};
use crate::SpeechState;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    SampleMic,
    SamplePlayback,
    Repeat,
    ReadClone,
    ReadVoice,
}

pub(crate) const BINDINGS: [(&str, Action); 5] = [
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

pub(crate) fn configure(mut bindings: Vec<(Action, Tracker)>) {
    let mut current = shortcuts().lock().unwrap();
    for (_, tracker) in &mut bindings {
        tracker.suspended = current.suspended;
    }
    current.bindings = bindings;
}

pub(crate) fn parse_bindings(
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
    fn event(&mut self, vk: u16, down: bool) -> (Option<Action>, bool) {
        let mut hit = None;
        let key = dictto_windows::keymap::from_vk(vk);
        for (action, tracker) in &mut self.bindings {
            let transition = key.and_then(|key| tracker.event_from(vk, key, down));
            if transition == Some(true) && hit.is_none() {
                hit = Some(*action);
            }
        }
        // Keep the function key and its repeats out of the focused application's accelerators.
        if hit.is_some() && !matches!(vk,0x10..=0x12|0x5b..=0x5c|0xa0..=0xa5) {
            self.suppressed.insert(vk);
        }
        let suppress = if down {
            self.suppressed.contains(&vk)
        } else {
            self.suppressed.remove(&vk)
        };
        (hit, suppress)
    }
}

pub(crate) fn key_event(vk: u16, down: bool) -> (Option<Action>, bool) {
    shortcuts().lock().unwrap().event(vk, down)
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
            state.mic_route.check_available()?;
        }
        let key = dictto_core::db::keystore::get_api_key()
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
                .map_err(|e| dictto_core::transcription::friendly_error(&e.to_string()))?
                .text
        } else {
            tokio::task::spawn_blocking(selection_or_clipboard)
                .await
                .map_err(|_| "Could not read selected text")??
        };
        if text.trim().is_empty() {
            return Err("No text to speak.".into());
        }
        let player = render::WindowsPlayer {
            microphone: use_virtual_mic.then(|| virtual_mic::Player::new(state.mic_route.clone())),
        };
        render::speak(
            &state,
            &settings,
            &key,
            &text,
            config.language.as_deref().unwrap_or(""),
            sample.as_deref(),
            &player,
        )
        .await
    }
}

fn replace_sample(path: &std::path::Path, wav: &[u8]) -> Result<(), String> {
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

fn preferred_text(
    selected: Option<String>,
    clipboard: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    selected
        .filter(|s| !s.trim().is_empty())
        .map(Ok)
        .unwrap_or_else(clipboard)
}

unsafe fn edit_selection(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::{
        Foundation::*,
        UI::{Controls::EM_GETSEL, WindowsAndMessaging::*},
    };
    let mut class = [0u16; 32];
    let len = GetClassNameW(hwnd, &mut class);
    if !String::from_utf16_lossy(&class[..len as usize]).eq_ignore_ascii_case("Edit")
        || GetWindowLongW(hwnd, GWL_STYLE) & ES_PASSWORD as i32 != 0
    {
        return None;
    }
    let send = |msg, w, l| {
        let mut result = 0;
        (SendMessageTimeoutW(
            hwnd,
            msg,
            WPARAM(w),
            LPARAM(l),
            SMTO_ABORTIFHUNG,
            250,
            Some(&mut result),
        )
        .0 != 0)
            .then_some(result)
    };
    let mut start = 0u32;
    let mut end = 0u32;
    send(
        EM_GETSEL,
        (&raw mut start) as usize,
        (&raw mut end) as isize,
    )?;
    if start >= end {
        return None;
    }
    let len = send(WM_GETTEXTLENGTH, 0, 0)?;
    if end as usize > len {
        return None;
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(len.checked_add(1)?).ok()?;
    buffer.resize(len + 1, 0u16);
    let copied = send(WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize)?;
    if (end as usize) > copied {
        return None;
    }
    Some(String::from_utf16_lossy(
        &buffer[start as usize..end as usize],
    ))
}

unsafe fn selection_from_element(
    automation: &windows::Win32::UI::Accessibility::IUIAutomation,
    mut element: windows::Win32::UI::Accessibility::IUIAutomationElement,
) -> windows::core::Result<String> {
    use windows::Win32::UI::Accessibility::*;
    let walker = automation.ControlViewWalker()?;
    loop {
        if element.CurrentIsPassword()?.as_bool() {
            break;
        }
        if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            if let Ok(ranges) = pattern.GetSelection() {
                let mut text = Vec::new();
                for i in 0..ranges.Length()? {
                    text.push(ranges.GetElement(i)?.GetText(-1)?.to_string());
                }
                let text = text.join("\n");
                if !text.trim().is_empty() {
                    return Ok(text);
                }
            }
        }
        if let Ok(hwnd) = element.CurrentNativeWindowHandle() {
            if let Some(text) = edit_selection(hwnd) {
                return Ok(text);
            }
        }
        let Ok(parent) = walker.GetParentElement(&element) else {
            break;
        };
        element = parent;
    }
    Ok(String::new())
}

fn selection_or_clipboard() -> Result<String, String> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Accessibility::*;
    let selection = unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let selection = (|| -> windows::core::Result<String> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            selection_from_element(&automation, automation.GetFocusedElement()?)
        })()
        .ok();
        if init.is_ok() {
            CoUninitialize();
        }
        selection
    };
    preferred_text(selection, || {
        arboard::Clipboard::new()
            .and_then(|mut c| c.get_text())
            .map_err(|_| "Select text or copy it to the clipboard first.".into())
    })
}

pub(crate) mod cache;
mod communications;
mod playback;
mod render;
pub(crate) mod session;
pub(crate) mod virtual_mic;

#[cfg(test)]
mod tests {
    use super::*;
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
    fn windows_edit_selection_is_read_without_copying() {
        use windows::core::w;
        use windows::Win32::{
            Foundation::*,
            System::Com::*,
            UI::{Accessibility::*, Controls::EM_SETSEL, WindowsAndMessaging::*},
        };
        let (ready, hwnd_rx) = std::sync::mpsc::channel();
        let window = std::thread::spawn(move || unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("EDIT"),
                w!("one two three"),
                WS_POPUP,
                0,
                0,
                180,
                40,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(4)), Some(LPARAM(7)));
            ready.send(hwnd.0 as usize).unwrap();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_APP {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = DestroyWindow(hwnd);
        });
        let hwnd = HWND(hwnd_rx.recv().unwrap() as _);
        let result = unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            let result = (|| -> windows::core::Result<String> {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                selection_from_element(&automation, automation.ElementFromHandle(hwnd)?)
            })();
            if init.is_ok() {
                CoUninitialize();
            }
            PostMessageW(Some(hwnd), WM_APP, WPARAM(0), LPARAM(0)).unwrap();
            result
        };
        window.join().unwrap();
        assert_eq!(result.unwrap(), "two");
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
        assert_eq!(chords.event(0xa2, true), (None, false));
        assert_eq!(chords.event(0xa0, true), (None, false));
        assert_eq!(chords.event(0x78, true), (Some(Action::ReadClone), true));
        assert_eq!(chords.event(0x78, true), (None, true));
        assert_eq!(chords.event(0xa2, false), (None, false));
        assert_eq!(chords.event(0x78, false), (None, true));
        assert_eq!(chords.event(0x78, true), (None, false));
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
