//! What the settings pages save and how they validate it, free of GTK so it is tested on
//! every platform. Rules and texts follow the Windows window (apps/native/src/ui.rs and
//! ui_speech.rs); every write goes through the daemon, which validates again.
use dictto_core::keys::Chord;
use dictto_core::speech::api::{default_speech_model, default_speech_voice, SpeechModel};
use serde_json::Value;
use std::collections::HashMap;

pub(crate) type Settings = HashMap<String, String>;

pub(crate) const SAVED: &str = "Saved";
pub(crate) const KEY_SAVED: &str = "API key saved in GNOME Keyring";
pub(crate) const KEY_REMOVED: &str = "API key removed";
const NUMBER_ERROR: &str = "Enter a valid whole number; recording limit must be positive.";
const INSTRUCTION_LIMIT: usize = 1024;

/// Windows says "Windows playback" for the loopback sample; here it is the desktop's output.
pub(crate) const SPEECH_SHORTCUTS: [(&str, &str); 5] = [
    ("tts_hotkey_sample_mic", "Record voice sample (microphone)"),
    ("tts_hotkey_sample_loopback", "Record voice sample (system playback)"),
    ("tts_hotkey_speech_to_speech", "Repeat my speech in the cloned voice"),
    ("tts_hotkey_read_clone", "Read text in the cloned voice"),
    ("tts_hotkey_read_voice", "Read text in the engine voice"),
];

/// The daemon calls the settings process needs to save; `Err` carries user-facing text.
pub(crate) trait Backend {
    async fn set_setting(&self, key: &str, value: &str) -> Result<(), String>;
    async fn save_key(&self, key: &str) -> Result<(), String>;
    async fn remove_key(&self) -> Result<(), String>;
}

pub(crate) fn validate(key: &str, value: &str) -> Result<(), String> {
    let invalid = match key {
        "recording_limit_seconds" => value.parse::<u64>().map_or(true, |v| v == 0),
        "paste_delay_ms" => value.parse::<u64>().is_err(),
        _ => false,
    };
    if invalid {
        Err(NUMBER_ERROR.into())
    } else {
        Ok(())
    }
}

pub(crate) async fn persist<B: Backend>(backend: &B, key: &str, value: &str) -> Result<(), String> {
    validate(key, value)?;
    backend.set_setting(key, value).await
}

pub(crate) fn bool_value(on: bool) -> &'static str {
    if on {
        "true"
    } else {
        "false"
    }
}

pub(crate) fn is_on(settings: &Settings, key: &str) -> bool {
    settings.get(key).is_some_and(|v| v == "true")
}

/// The stored form of a shortcut (`Ctrl+Shift+F8`), rejected when it overlaps another
/// configured shortcut by the core's rules. An empty speech shortcut disables it.
pub(crate) fn shortcut_value(settings: &Settings, key: &str, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let value = if raw.is_empty() && key != "hotkey" {
        String::new()
    } else {
        Chord::parse(raw)?.display()
    };
    let mut merged = settings.clone();
    merged.insert(key.into(), value.clone());
    dictto_core::speech::parse_bindings(&merged, true)?;
    Ok(value)
}

pub(crate) async fn save_shortcut<B: Backend>(
    backend: &B,
    settings: &Settings,
    key: &str,
    raw: &str,
) -> Result<String, String> {
    let value = shortcut_value(settings, key, raw)?;
    persist(backend, key, &value).await?;
    Ok(value)
}

pub(crate) fn languages(settings: &Settings) -> Vec<String> {
    serde_json::from_str(settings.get("languages").map_or("[]", String::as_str)).unwrap_or_default()
}

pub(crate) fn auto_detect(settings: &Settings) -> bool {
    let values = languages(settings);
    values.is_empty() || values.iter().any(|v| v == "auto")
}

pub(crate) fn languages_summary(settings: &Settings) -> String {
    if auto_detect(settings) {
        "Auto-detect".into()
    } else {
        languages(settings).join(", ")
    }
}

/// The selected languages after a change in the list, ordered as Windows does: languages
/// hidden by the search filter keep their place, then the checked visible ones follow in
/// list order. `visible` and `checked` are in list order.
pub(crate) fn selected_languages(current: &[String], visible: &[&str], checked: &[&str]) -> Vec<String> {
    let mut values: Vec<String> =
        current.iter().filter(|v| *v != "auto" && !visible.contains(&v.as_str())).cloned().collect();
    values.extend(checked.iter().map(|c| c.to_string()));
    values
}

pub(crate) fn languages_value(values: &[String]) -> String {
    serde_json::to_string(values).expect("strings serialize")
}

pub(crate) fn language_matches(code: &str, name: &str, filter: &str) -> bool {
    let filter = filter.to_lowercase();
    name.to_lowercase().contains(&filter) || code.contains(&filter)
}

/// `Microphones()` row: id, name, form factor, is the default.
pub(crate) type Microphone = (String, String, String, bool);

pub(crate) struct MicrophoneChoices {
    pub labels: Vec<String>,
    /// Stored `microphone_device` values, parallel to `labels`; Windows stores the name too.
    pub values: Vec<String>,
    pub selected: usize,
}

/// Windows labels the first entry with the device auto-detect currently uses; here that
/// is PipeWire's default source.
pub(crate) fn microphone_choices(microphones: &[Microphone], settings: &Settings) -> MicrophoneChoices {
    let current = microphones.iter().find(|m| m.3).map(|m| m.1.as_str()).unwrap_or("");
    let mut labels = vec![if current.is_empty() {
        "Auto-detect".to_string()
    } else {
        format!("Auto-detect ({current})")
    }];
    let mut values = vec!["auto-detect".to_string()];
    for (_, name, _, _) in microphones {
        labels.push(name.clone());
        values.push(name.clone());
    }
    let selected = values
        .iter()
        .position(|v| Some(v) == settings.get("microphone_device"))
        .unwrap_or(0);
    MicrophoneChoices { labels, values, selected }
}

pub(crate) async fn save_key<B: Backend>(backend: &B, raw: &str) -> Result<&'static str, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Err("Enter an API key before saving".into());
    }
    backend.save_key(key).await.map(|()| KEY_SAVED)
}

pub(crate) async fn save_models<B: Backend>(backend: &B, transcription: &str, cleanup: &str) -> Result<(), String> {
    persist(backend, "transcription_model", transcription).await?;
    persist(backend, "llm_model", cleanup).await
}

/// Status text for daemon events the Windows window also reports.
pub(crate) fn event_status(name: &str, payload: &Value) -> Option<String> {
    use dictto_core::events;
    match name {
        events::SPEECH_ERROR => Some(payload["message"].as_str().unwrap_or("Speech action failed.").into()),
        // The daemon already made dictation errors readable before the bus.
        events::DICTATION_ERROR => Some(payload["message"].as_str().unwrap_or_default().into()),
        "error" | "recording-error" => Some(
            dictto_core::transcription::friendly_error(payload["message"].as_str().unwrap_or_default()).into(),
        ),
        events::RECORDING_STATE_CHANGED => {
            Some(payload["state"].as_str().or_else(|| payload.as_str()).unwrap_or("").into())
        }
        _ => None,
    }
}

/// Speech models discovered through the daemon; `generation` drops stale answers.
#[derive(Default)]
pub(crate) struct Catalog {
    pub models: Vec<SpeechModel>,
    pub generation: u64,
    pub loading: bool,
}

pub(crate) const LOADING_MODELS: &str = "Loading speech models from the configured API…";

impl Catalog {
    pub fn begin_refresh(&mut self) -> u64 {
        self.generation += 1;
        self.loading = true;
        self.generation
    }

    /// A new API base URL makes the current list meaningless.
    pub fn invalidate(&mut self) {
        self.models.clear();
        self.generation += 1;
        self.loading = false;
    }

    /// `None` for an answer to an older request.
    pub fn receive(&mut self, payload: &Value) -> Option<Result<(), String>> {
        if payload["generation"].as_u64() != Some(self.generation) {
            return None;
        }
        self.loading = false;
        self.models.clear();
        if let Some(error) = payload["error"].as_str() {
            return Some(Err(format!("Speech discovery: {error}")));
        }
        Some(match serde_json::from_value::<Vec<SpeechModel>>(payload["models"].clone()) {
            Ok(models) if models.is_empty() => Err("This API advertises no speech models.".into()),
            Ok(models) => {
                self.models = models;
                Ok(())
            }
            Err(e) => Err(format!("Invalid speech catalog: {e}")),
        })
    }

    pub fn selected<'a>(&'a self, settings: &Settings) -> Option<&'a SpeechModel> {
        self.models.iter().find(|m| Some(&m.id) == settings.get("tts_model"))
    }
}

/// After a successful discovery: keeps a chosen model, otherwise picks the core's
/// default model and its default speaker, as Windows does. Returns the status text.
pub(crate) async fn adopt_catalog<B: Backend>(backend: &B, models: &[SpeechModel], settings: &Settings) -> String {
    if settings.get("tts_model").is_some_and(|id| !id.is_empty()) {
        return "Speech models loaded.".into();
    }
    let Some(model) = default_speech_model(models) else {
        return "This API advertises no speech models.".into();
    };
    let voice_key = format!("tts_voice:{}", model.id);
    let instruction_key = format!("tts_voice_instruction:{}", model.id);
    if !settings.contains_key(&voice_key) && !settings.contains_key(&instruction_key) {
        if let Some(speaker) = default_speech_voice(model) {
            if let Err(error) = persist(backend, &voice_key, &speaker.to_string()).await {
                return error;
            }
        }
    }
    match persist(backend, "tts_model", &model.id).await {
        Ok(()) => format!("Selected {}.", model.id),
        Err(error) => error,
    }
}

/// Everything the Speech page shows, derived like Windows' `render_speech`.
#[derive(Debug, PartialEq)]
pub(crate) struct SpeechView {
    pub model_labels: Vec<String>,
    pub model_index: usize,
    pub model_enabled: bool,
    pub refresh_enabled: bool,
    pub voice_labels: Vec<String>,
    pub voice_index: usize,
    pub voice_enabled: bool,
    pub instruction: String,
    pub instruction_enabled: bool,
    pub modes: String,
    pub engine: String,
    pub engine_enabled: bool,
    pub engine_hint: String,
    pub virtual_mic: bool,
}

/// `drafts` keeps unsaved per-model texts (`tts_voice_instruction:<id>`,
/// `engine_draft:<id>`) while the user switches models.
pub(crate) fn speech_view(catalog: &Catalog, settings: &Settings, drafts: &Settings) -> SpeechView {
    let model = catalog.selected(settings);
    let first = match settings.get("tts_model").filter(|s| !s.is_empty()) {
        Some(id) if model.is_none() => format!("{id} (not discovered)"),
        _ if catalog.loading => "Loading models…".into(),
        _ if catalog.models.is_empty() => "No models available".into(),
        _ => "Choose a model".into(),
    };
    let mut model_labels = vec![first];
    model_labels.extend(catalog.models.iter().map(|m| format!("{} ({})", m.name, m.id)));
    let model_index = catalog
        .models
        .iter()
        .position(|m| Some(&m.id) == settings.get("tts_model"))
        .map_or(0, |i| i + 1);
    let has_field = |field: &str| {
        model.is_some_and(|m| m.capabilities.voice_fields.values().flatten().any(|f| f == field))
    };
    let mut voice_labels = vec!["No numbered speaker".to_string()];
    let mut voice_index = 0;
    if let Some(m) = model {
        let saved = settings.get(&format!("tts_voice:{}", m.id)).and_then(|s| s.parse::<i64>().ok());
        for v in &m.capabilities.voices {
            if saved == Some(v.id) {
                voice_index = voice_labels.len();
            }
            voice_labels.push(format!("{} · {}", v.id, v.name));
        }
    }
    let id = model.map_or("", |m| m.id.as_str());
    let instruction_key = format!("tts_voice_instruction:{id}");
    let instruction = drafts
        .get(&instruction_key)
        .or_else(|| settings.get(&instruction_key))
        .cloned()
        .unwrap_or_default();
    let engine = match model {
        Some(m) => drafts.get(&format!("engine_draft:{id}")).cloned().unwrap_or_else(|| {
            settings
                .get("tts_engine_options")
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|all| all.get(&m.id).map(Value::to_string))
                .unwrap_or_else(|| "{}".into())
        }),
        None => String::new(),
    };
    let schema = model.map(|m| &m.capabilities.engine_options);
    SpeechView {
        model_enabled: !catalog.models.is_empty() && !catalog.loading,
        refresh_enabled: !catalog.loading,
        voice_enabled: has_field("voice") && voice_labels.len() > 1,
        voice_labels,
        voice_index,
        instruction,
        instruction_enabled: has_field("voice_instruction"),
        modes: format!(
            "Available modes: {}",
            model.map(|m| m.capabilities.modes.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()
        ),
        engine,
        engine_enabled: schema.is_some_and(|s| !s.is_empty()),
        engine_hint: schema
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k} ({v})"))
            .collect::<Vec<_>>()
            .join(", "),
        model_labels,
        model_index,
        virtual_mic: is_on(settings, "tts_virtual_mic"),
    }
}

/// `tts_model` value for an entry of the model list; the first entry clears it.
pub(crate) fn model_at(catalog: &Catalog, index: usize) -> String {
    index.checked_sub(1).and_then(|i| catalog.models.get(i)).map(|m| m.id.clone()).unwrap_or_default()
}

/// `tts_voice:<model>` value for an entry of the voice list; the first entry clears it.
pub(crate) fn voice_at(model: &SpeechModel, index: usize) -> String {
    index
        .checked_sub(1)
        .and_then(|i| model.capabilities.voices.get(i))
        .map(|v| v.id.to_string())
        .unwrap_or_default()
}

pub(crate) async fn save_instruction<B: Backend>(backend: &B, model: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > INSTRUCTION_LIMIT {
        return Err("Voice instructions are limited to 1,024 characters.".into());
    }
    persist(backend, &format!("tts_voice_instruction:{model}"), value).await
}

/// The new `tts_engine_options` value with `draft` as this model's group, checked
/// against the model's advertised option types.
pub(crate) fn engine_options(model: &SpeechModel, draft: &str, saved: &str) -> Result<String, String> {
    let group: serde_json::Map<String, Value> =
        serde_json::from_str(draft).map_err(|e| format!("Expected a JSON object: {e}"))?;
    for (name, value) in &group {
        let kind = model
            .capabilities
            .engine_options
            .get(name)
            .ok_or_else(|| format!("Unknown {} option: {name}", model.id))?;
        let valid = match kind.as_str() {
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            _ => false,
        };
        if !valid {
            return Err(format!("{name} must be {kind}"));
        }
    }
    let mut groups: serde_json::Map<String, Value> =
        serde_json::from_str(saved).map_err(|e| format!("Invalid saved options: {e}"))?;
    groups.insert(model.id.clone(), Value::Object(group));
    Ok(Value::Object(groups).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        writes: RefCell<Vec<(String, String)>>,
        keys: RefCell<Vec<String>>,
    }

    impl Backend for Fake {
        async fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
            // The daemon's own check, so a test sees what reaches the bus.
            if key == "hotkey" {
                Chord::parse(value)?;
            }
            self.writes.borrow_mut().push((key.into(), value.into()));
            Ok(())
        }
        async fn save_key(&self, key: &str) -> Result<(), String> {
            self.keys.borrow_mut().push(key.into());
            Ok(())
        }
        async fn remove_key(&self) -> Result<(), String> {
            Ok(())
        }
    }

    impl Fake {
        fn writes(&self) -> Vec<(String, String)> {
            self.writes.borrow().clone()
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn settings(list: &[(&str, &str)]) -> Settings {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn save_general_writes_expected_keys() {
        let fake = Fake::default();
        let stored = settings(&[("hotkey", "Ctrl+Win"), ("languages", r#"["en"]"#)]);
        block_on(async {
            persist(&fake, "auto_start", bool_value(true)).await.unwrap();
            persist(&fake, "sound_effects_enabled", bool_value(false)).await.unwrap();
            let languages = selected_languages(&languages(&stored), &["en", "fr"], &["en", "fr"]);
            persist(&fake, "languages", &languages_value(&languages)).await.unwrap();
            assert_eq!(save_shortcut(&fake, &stored, "hotkey", " ctrl+shift+f8 ").await.unwrap(), "Ctrl+Shift+F8");
        });
        assert_eq!(
            fake.writes(),
            pairs(&[
                ("auto_start", "true"),
                ("sound_effects_enabled", "false"),
                ("languages", r#"["en","fr"]"#),
                ("hotkey", "Ctrl+Shift+F8"),
            ])
        );
        // An invalid chord never reaches the daemon.
        let error = block_on(save_shortcut(&fake, &stored, "hotkey", "Ctrl")).unwrap_err();
        assert_eq!(error, "Use modifiers with one key, two modifiers, or a function key");
        assert_eq!(fake.writes().len(), 4);
    }

    #[test]
    fn shortcut_conflict_is_rejected() {
        let fake = Fake::default();
        let stored = settings(&[("hotkey", "Ctrl+Win"), ("tts_hotkey_read_clone", "Ctrl+Alt+F8")]);
        let conflict = "Shortcuts must be distinct and cannot contain another configured shortcut.";
        // Contains the dictation chord.
        assert_eq!(block_on(save_shortcut(&fake, &stored, "tts_hotkey_read_voice", "Ctrl+Win+R")).unwrap_err(), conflict);
        // Same chord as another speech shortcut, typed differently.
        assert_eq!(block_on(save_shortcut(&fake, &stored, "tts_hotkey_sample_mic", "alt+ctrl+F8")).unwrap_err(), conflict);
        // The dictation chord cannot swallow a speech shortcut either.
        assert_eq!(block_on(save_shortcut(&fake, &stored, "hotkey", "Ctrl+Alt")).unwrap_err(), conflict);
        assert!(fake.writes().is_empty());
        // Clearing disables a speech shortcut; the dictation hotkey cannot be cleared.
        assert_eq!(block_on(save_shortcut(&fake, &stored, "tts_hotkey_read_clone", "  ")).unwrap(), "");
        assert!(block_on(save_shortcut(&fake, &stored, "hotkey", "")).is_err());
        assert_eq!(fake.writes(), pairs(&[("tts_hotkey_read_clone", "")]));
    }

    #[test]
    fn numbers_are_checked_before_saving() {
        let fake = Fake::default();
        for (key, value) in [("recording_limit_seconds", "0"), ("recording_limit_seconds", "x"), ("paste_delay_ms", "-1")] {
            assert!(block_on(persist(&fake, key, value)).unwrap_err().contains("positive"));
        }
        block_on(persist(&fake, "paste_delay_ms", "0")).unwrap();
        block_on(persist(&fake, "recording_limit_seconds", "120")).unwrap();
        assert_eq!(fake.writes(), pairs(&[("paste_delay_ms", "0"), ("recording_limit_seconds", "120")]));
    }

    #[test]
    fn api_key_and_models() {
        let fake = Fake::default();
        assert_eq!(block_on(save_key(&fake, "  ")).unwrap_err(), "Enter an API key before saving");
        assert_eq!(block_on(save_key(&fake, " sk-1 ")).unwrap(), KEY_SAVED);
        assert_eq!(*fake.keys.borrow(), ["sk-1"]);
        block_on(save_models(&fake, "whisper-1", "gpt-4o-mini")).unwrap();
        assert_eq!(fake.writes(), pairs(&[("transcription_model", "whisper-1"), ("llm_model", "gpt-4o-mini")]));
    }

    #[test]
    fn languages_toggle_and_summarize() {
        let auto = settings(&[("languages", r#"["auto"]"#)]);
        assert!(auto_detect(&auto));
        assert_eq!(languages_summary(&auto), "Auto-detect");
        assert_eq!(selected_languages(&languages(&auto), &["en", "fr"], &["fr"]), ["fr"]);
        let two = settings(&[("languages", r#"["fr","en"]"#)]);
        assert_eq!(languages_summary(&two), "fr, en");
        assert_eq!(selected_languages(&languages(&two), &["en", "fr"], &["en"]), ["en"]);
        // Filtered to French: English is hidden and keeps its place ahead of the visible ones.
        let three = settings(&[("languages", r#"["fr","en","de"]"#)]);
        assert_eq!(selected_languages(&languages(&three), &["fr"], &["fr"]), ["en", "de", "fr"]);
        assert_eq!(selected_languages(&languages(&three), &["en", "de", "fr"], &["en", "fr"]), ["en", "fr"]);
        assert!(language_matches("fr", "French", "fren"));
        assert!(language_matches("yue", "Cantonese", "yu"));
        assert!(!language_matches("fr", "French", "german"));
    }

    #[test]
    fn microphones_list_auto_detect_first() {
        let mics = vec![
            ("1".to_string(), "Desk".to_string(), "Microphone".to_string(), false),
            ("2".to_string(), "Headset".to_string(), "Headset".to_string(), true),
        ];
        let choices = microphone_choices(&mics, &settings(&[("microphone_device", "Desk")]));
        assert_eq!(choices.labels, ["Auto-detect (Headset)", "Desk", "Headset"]);
        assert_eq!(choices.values, ["auto-detect", "Desk", "Headset"]);
        assert_eq!(choices.selected, 1);
        assert_eq!(microphone_choices(&[], &Settings::new()).labels, ["Auto-detect"]);
    }

    fn catalog() -> Value {
        json!([
            {"id":"qwen3-tts","name":"Qwen","capabilities":{
                "modes":{"speaker":["worker"]},"voice_fields":{"speaker":["voice","voice_instruction"]},
                "voices":[{"id":18,"name":"Test speaker"}],"engine_options":{"temperature":"number"}
            }},
            {"id":"omnivoice","name":"Omni","capabilities":{
                "modes":{"auto":["worker"]},"voice_fields":{"auto":[]},"engine_options":{"num_step":"integer"}
            }}
        ])
    }

    #[test]
    fn speech_catalog_selects_a_default_and_drops_stale_answers() {
        let mut list = Catalog::default();
        let generation = list.begin_refresh();
        let view = speech_view(&list, &Settings::new(), &Settings::new());
        assert_eq!(view.model_labels, ["Loading models…"]);
        assert!(!view.model_enabled && !view.refresh_enabled);
        assert!(list.receive(&json!({"generation": generation + 1, "models": []})).is_none());
        assert_eq!(list.receive(&json!({"generation": generation, "models": catalog()})), Some(Ok(())));

        let fake = Fake::default();
        assert_eq!(block_on(adopt_catalog(&fake, &list.models, &Settings::new())), "Selected qwen3-tts.");
        assert_eq!(fake.writes(), pairs(&[("tts_voice:qwen3-tts", "18"), ("tts_model", "qwen3-tts")]));

        let stored = settings(&[("tts_model", "qwen3-tts"), ("tts_voice:qwen3-tts", "18")]);
        let view = speech_view(&list, &stored, &Settings::new());
        assert_eq!(view.model_labels, ["Choose a model", "Qwen (qwen3-tts)", "Omni (omnivoice)"]);
        assert_eq!((view.model_index, view.voice_index), (1, 1));
        assert_eq!(view.voice_labels, ["No numbered speaker", "18 · Test speaker"]);
        assert!(view.voice_enabled && view.instruction_enabled && view.engine_enabled);
        assert_eq!(view.engine, "{}");
        assert_eq!(view.engine_hint, "temperature (number)");
        assert_eq!(view.modes, "Available modes: speaker");
        assert_eq!(model_at(&list, 2), "omnivoice");
        assert_eq!(model_at(&list, 0), "");
        assert_eq!(voice_at(&list.models[0], 0), "");

        list.begin_refresh();
        let generation = list.generation;
        assert_eq!(
            list.receive(&json!({"generation": generation, "error": "unauthorized"})),
            Some(Err("Speech discovery: unauthorized".into()))
        );
        let view = speech_view(&list, &stored, &Settings::new());
        assert_eq!(view.model_labels, ["qwen3-tts (not discovered)"]);
        assert!(!view.voice_enabled && !view.engine_enabled);
    }

    #[test]
    fn engine_options_follow_the_schema() {
        let models: Vec<SpeechModel> = serde_json::from_value(catalog()).unwrap();
        assert_eq!(engine_options(&models[0], r#"{"temperature":"bad"}"#, "{}").unwrap_err(), "temperature must be number");
        assert!(engine_options(&models[0], r#"{"steps":1}"#, "{}").unwrap_err().starts_with("Unknown qwen3-tts option"));
        assert_eq!(
            engine_options(&models[0], r#"{"temperature":0.8}"#, r#"{"omnivoice":{"num_step":4}}"#).unwrap(),
            r#"{"omnivoice":{"num_step":4},"qwen3-tts":{"temperature":0.8}}"#
        );
        let fake = Fake::default();
        let long = "x".repeat(1025);
        assert!(block_on(save_instruction(&fake, "qwen3-tts", &long)).unwrap_err().contains("1,024"));
        block_on(save_instruction(&fake, "qwen3-tts", "Softly")).unwrap();
        assert_eq!(fake.writes(), pairs(&[("tts_voice_instruction:qwen3-tts", "Softly")]));
    }

    #[test]
    fn events_become_status_text() {
        use dictto_core::events;
        assert_eq!(event_status(events::SPEECH_ERROR, &json!({"message":"No voice"})).unwrap(), "No voice");
        assert_eq!(event_status(events::RECORDING_STATE_CHANGED, &json!({"state":"recording"})).unwrap(), "recording");
        assert_eq!(event_status(events::WAVEFORM_DATA, &json!({})), None);
    }
}
