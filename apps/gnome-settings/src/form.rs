//! What the settings pages save and how they validate it, free of GTK so it is tested on
//! every platform. Rules and texts follow the Windows window (apps/native/src/ui.rs,
//! ui_api.rs, ui_keys.rs and ui_speech.rs); every write goes through the daemon, which
//! validates again.
use dictto_core::keys::Chord;
use dictto_core::settings_model::LANGUAGES;
use dictto_core::speech::api::{default_speech_model, default_speech_voice, SpeechModel};
use serde_json::Value;
use std::collections::HashMap;

pub(crate) type Settings = HashMap<String, String>;

pub(crate) const SAVED: &str = "Saved";
pub(crate) const KEY_SAVED: &str = "API key saved.";
pub(crate) const KEY_REMOVED: &str = "API key removed.";
const INSTRUCTION_LIMIT: usize = 1024;
pub(crate) const DEFAULT_HOTKEY: &str = "Ctrl+Win";

pub(crate) const ENDPOINTS: &[&str] = &["https://api.openai.com/v1", "https://api.groq.com/openai/v1"];
pub(crate) const LIMITS: &[(&str, &str)] = &[
    ("60", "1 minute"),
    ("120", "2 minutes"),
    ("300", "5 minutes"),
    ("600", "10 minutes"),
    ("1800", "30 minutes"),
];
pub(crate) const DELAYS: &[(&str, &str)] =
    &[("0", "None"), ("50", "50 ms"), ("150", "150 ms"), ("300", "300 ms"), ("500", "500 ms")];

/// Windows says "Windows playback" for the loopback sample; here it is the desktop's output.
pub(crate) const SHORTCUTS: [(&str, &str); 6] = [
    ("hotkey", "Dictate (hold; double-press to keep recording)"),
    ("tts_hotkey_sample_mic", "Record a voice sample from the microphone"),
    ("tts_hotkey_sample_loopback", "Record a voice sample from system playback"),
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

pub(crate) async fn persist<B: Backend>(backend: &B, key: &str, value: &str) -> Result<(), String> {
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

/// A list that saves a setting when an entry is picked.
#[derive(Debug, PartialEq)]
pub(crate) struct Choices {
    pub labels: Vec<String>,
    /// Stored values, parallel to `labels`.
    pub values: Vec<String>,
    pub selected: usize,
}

/// A saved value missing from `options` stays first, labelled with `missing`, so filling
/// the list never changes the setting by itself.
pub(crate) fn choices(mut options: Vec<(String, String)>, saved: &str, missing: &str) -> Choices {
    if !options.iter().any(|(value, _)| value == saved) {
        let label = if missing.is_empty() { saved.to_string() } else { format!("{saved} {missing}") };
        options.insert(0, (saved.to_string(), label));
    }
    let selected = options.iter().position(|(value, _)| value == saved).unwrap_or(0);
    let (values, labels) = options.into_iter().unzip();
    Choices { labels, values, selected }
}

pub(crate) fn preset_choices(presets: &[(&str, &str)], settings: &Settings, key: &str, unit: &str) -> Choices {
    let options = presets.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
    choices(options, settings.get(key).map_or("", String::as_str), unit)
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

/// What Reset (dictation) or Clear (speech shortcuts) stores.
pub(crate) fn shortcut_reset(key: &str) -> &'static str {
    if key == "hotkey" {
        DEFAULT_HOTKEY
    } else {
        ""
    }
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
        return "Auto-detect".into();
    }
    languages(settings)
        .iter()
        .map(|code| LANGUAGES.iter().find(|(c, _)| c == code).map_or(code.as_str(), |(_, name)| name))
        .collect::<Vec<_>>()
        .join(", ")
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

/// Windows labels the first entry with the device auto-detect currently uses; here that
/// is PipeWire's default source. Windows stores the device name too.
pub(crate) fn microphone_choices(microphones: &[Microphone], settings: &Settings) -> Choices {
    let current = microphones.iter().find(|m| m.3).map(|m| m.1.as_str()).unwrap_or("");
    let auto = if current.is_empty() { "Auto-detect".to_string() } else { format!("Auto-detect ({current})") };
    let mut options = vec![("auto-detect".to_string(), auto)];
    options.extend(microphones.iter().map(|m| (m.1.clone(), m.1.clone())));
    choices(options, settings.get("microphone_device").map_or("", String::as_str), "(not connected)")
}

pub(crate) async fn save_key<B: Backend>(backend: &B, raw: &str) -> Result<&'static str, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Err("Paste an API key before saving.".into());
    }
    backend.save_key(key).await.map(|()| KEY_SAVED)
}

/// The endpoint to save, or `None` when the typed text is empty or already saved.
pub(crate) fn endpoint_change(settings: &Settings, raw: &str) -> Option<String> {
    let url = raw.trim();
    (!url.is_empty() && Some(url) != settings.get("api_base_url").map(String::as_str)).then(|| url.to_string())
}

/// The endpoint presets with a custom saved URL first.
pub(crate) fn endpoint_presets(settings: &Settings) -> Vec<String> {
    let saved = settings.get("api_base_url").map_or("", String::as_str);
    let mut urls: Vec<String> = ENDPOINTS.iter().map(|u| u.to_string()).collect();
    if !saved.is_empty() && !ENDPOINTS.contains(&saved) {
        urls.insert(0, saved.to_string());
    }
    urls
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

/// Transcription and chat models the endpoint advertised, through `DiscoverModels`.
#[derive(Default)]
pub(crate) struct Models {
    pub transcription: Vec<String>,
    pub chat: Vec<String>,
    pub generation: u64,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
}

impl Models {
    pub fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.generation
    }

    /// A reply still in flight belongs to the old endpoint or key.
    pub fn forget(&mut self) {
        *self = Self { generation: self.generation + 1, ..Default::default() };
    }

    /// `false` for an answer to an older request. A failed load keeps the previous lists.
    pub fn receive(&mut self, payload: &Value) -> bool {
        if payload["generation"].as_u64() != Some(self.generation) {
            return false;
        }
        self.loading = false;
        if let Some(error) = payload["error"].as_str() {
            self.error = Some(error.into());
        } else {
            let list = |key: &str| serde_json::from_value(payload[key].clone()).unwrap_or_default();
            self.transcription = list("transcription");
            self.chat = list("chat");
            self.loaded = true;
        }
        true
    }

    /// The saved model stays first, marked once a list arrived, when the endpoint does not
    /// advertise it, so loading a catalog never switches models by itself.
    pub fn choices(&self, models: &[String], settings: &Settings, key: &str) -> Choices {
        let missing = if self.loaded { "(not advertised)" } else { "" };
        let options = models.iter().map(|m| (m.clone(), m.clone())).collect();
        choices(options, settings.get(key).map_or("", String::as_str), missing)
    }

    pub fn status(&self, has_key: bool) -> String {
        if !has_key {
            "Add an API key to load the available models.".into()
        } else if self.loading {
            "Checking the endpoint...".into()
        } else if let Some(error) = &self.error {
            format!("Could not list models: {error}")
        } else if self.loaded {
            format!(
                "Connected: {} transcription and {} chat models available.",
                self.transcription.len(),
                self.chat.len()
            )
        } else {
            String::new()
        }
    }
}

/// Speech models discovered through the daemon; `generation` drops stale answers.
#[derive(Default)]
pub(crate) struct Catalog {
    pub models: Vec<SpeechModel>,
    pub generation: u64,
    pub loading: bool,
}

impl Catalog {
    pub fn begin_refresh(&mut self) -> u64 {
        self.generation += 1;
        self.loading = true;
        self.generation
    }

    /// A new endpoint or key makes the current list meaningless.
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
            return Some(Err(format!("Speech models: {error}")));
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
/// default model and its default speaker, as Windows does. Returns news for the status
/// line; loading happens on every open, so keeping the model is not news.
pub(crate) async fn adopt_catalog<B: Backend>(
    backend: &B,
    models: &[SpeechModel],
    settings: &Settings,
) -> Option<String> {
    if settings.get("tts_model").is_some_and(|id| !id.is_empty()) {
        return None;
    }
    let model = default_speech_model(models)?;
    let voice_key = format!("tts_voice:{}", model.id);
    let instruction_key = format!("tts_voice_instruction:{}", model.id);
    if !settings.contains_key(&voice_key) && !settings.contains_key(&instruction_key) {
        if let Some(speaker) = default_speech_voice(model) {
            if let Err(error) = persist(backend, &voice_key, &speaker.to_string()).await {
                return Some(error);
            }
        }
    }
    Some(match persist(backend, "tts_model", &model.id).await {
        Ok(()) => format!("Selected speech model {}.", model.id),
        Err(error) => error,
    })
}

/// One typed control per engine option the model advertises, sorted by name.
#[derive(Debug, PartialEq)]
pub(crate) struct EngineField {
    pub name: String,
    pub kind: String,
    /// Typed text, or `true`/`false`/empty for a boolean.
    pub value: String,
}

/// Everything the Voice page shows, derived like Windows' `render_speech`.
#[derive(Debug, PartialEq)]
pub(crate) struct SpeechView {
    pub model_labels: Vec<String>,
    pub model_index: usize,
    pub model_enabled: bool,
    pub refresh_label: &'static str,
    pub refresh_enabled: bool,
    pub voice_labels: Vec<String>,
    pub voice_index: usize,
    pub voice_enabled: bool,
    pub instruction: String,
    pub instruction_enabled: bool,
    pub engine: Vec<EngineField>,
    pub virtual_mic: bool,
}

/// `drafts` keeps unsaved per-model texts (`tts_voice_instruction:<id>`,
/// `engine_draft:<id>:<name>`) while the user switches models.
pub(crate) fn speech_view(catalog: &Catalog, settings: &Settings, drafts: &Settings, has_key: bool) -> SpeechView {
    let model = catalog.selected(settings);
    let first = match settings.get("tts_model").filter(|s| !s.is_empty()) {
        Some(id) if model.is_none() && !catalog.loading => format!("{id} (not advertised)"),
        _ if catalog.loading => "Loading models...".into(),
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
    let mut voice_labels = vec!["Engine default".to_string()];
    let mut voice_index = 0;
    if let Some(m) = model {
        let saved = settings.get(&format!("tts_voice:{}", m.id)).and_then(|s| s.parse::<i64>().ok());
        for v in &m.capabilities.voices {
            if saved == Some(v.id) {
                voice_index = voice_labels.len();
            }
            voice_labels.push(format!("{} ({})", v.name, v.id));
        }
    }
    let id = model.map_or("", |m| m.id.as_str());
    let instruction_key = format!("tts_voice_instruction:{id}");
    let instruction = match model {
        Some(_) => drafts.get(&instruction_key).or_else(|| settings.get(&instruction_key)).cloned().unwrap_or_default(),
        None => String::new(),
    };
    let saved = saved_engine_options(settings, id);
    let engine = model
        .into_iter()
        .flat_map(|m| &m.capabilities.engine_options)
        .map(|(name, kind)| EngineField {
            name: name.clone(),
            kind: kind.clone(),
            value: drafts
                .get(&format!("engine_draft:{id}:{name}"))
                .cloned()
                .or_else(|| saved.get(name).map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)))
                .unwrap_or_default(),
        })
        .collect();
    SpeechView {
        model_enabled: !catalog.models.is_empty() && !catalog.loading,
        refresh_label: if catalog.loading { "Loading..." } else { "Refresh" },
        refresh_enabled: !catalog.loading && has_key,
        voice_enabled: has_field("voice") && voice_labels.len() > 1,
        voice_labels,
        voice_index,
        instruction,
        instruction_enabled: has_field("voice_instruction"),
        engine,
        model_labels,
        model_index,
        virtual_mic: is_on(settings, "tts_virtual_mic"),
    }
}

fn saved_engine_options(settings: &Settings, model: &str) -> serde_json::Map<String, Value> {
    settings
        .get("tts_engine_options")
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|all| all.get(model).and_then(Value::as_object).cloned())
        .unwrap_or_default()
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

/// The instruction to save, or `None` when unchanged.
pub(crate) fn instruction_change(settings: &Settings, model: &str, value: &str) -> Result<Option<String>, String> {
    if value.chars().count() > INSTRUCTION_LIMIT {
        return Err("Voice instructions are limited to 1,024 characters.".into());
    }
    let key = format!("tts_voice_instruction:{model}");
    Ok((settings.get(&key).map_or("", String::as_str) != value).then_some(key))
}

/// Boolean options are lists: Default, On, Off.
pub(crate) const BOOLEAN_LABELS: [&str; 3] = ["Default", "On", "Off"];

pub(crate) fn boolean_index(value: &str) -> u32 {
    match value {
        "true" => 1,
        "false" => 2,
        _ => 0,
    }
}

pub(crate) fn boolean_at(index: u32) -> &'static str {
    match index {
        1 => "true",
        2 => "false",
        _ => "",
    }
}

/// The new `tts_engine_options` value from the typed `(name, kind, text)` fields, or
/// `None` when nothing changed. An empty field leaves the option to the engine; when all
/// are empty the model's group is removed.
pub(crate) fn engine_options(
    model: &SpeechModel,
    fields: &[(String, String, String)],
    saved: &str,
) -> Result<Option<String>, String> {
    let mut group = serde_json::Map::new();
    for (name, kind, text) in fields {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let value = match kind.as_str() {
            "integer" => text.parse::<i64>().ok().map(Value::from),
            "number" => text.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number),
            "boolean" => Some(Value::Bool(text == "true")),
            _ => Some(Value::String(text.into())),
        };
        let value = value.ok_or_else(|| format!("{name} must be {kind}."))?;
        group.insert(name.clone(), value);
    }
    if let Some(name) = group.keys().find(|name| !model.capabilities.engine_options.contains_key(*name)) {
        return Err(format!("Unknown {} option: {name}", model.id));
    }
    let saved = if saved.is_empty() { "{}" } else { saved };
    let mut groups: serde_json::Map<String, Value> =
        serde_json::from_str(saved).map_err(|e| format!("Invalid saved options: {e}"))?;
    if group.is_empty() {
        groups.remove(&model.id);
    } else {
        groups.insert(model.id.clone(), Value::Object(group));
    }
    let options = Value::Object(groups).to_string();
    Ok((options != saved).then_some(options))
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
    fn shortcuts_save_in_stored_form() {
        let fake = Fake::default();
        let stored = settings(&[("hotkey", "Ctrl+Win")]);
        assert_eq!(block_on(save_shortcut(&fake, &stored, "hotkey", " ctrl+shift+f8 ")).unwrap(), "Ctrl+Shift+F8");
        assert_eq!(fake.writes(), pairs(&[("hotkey", "Ctrl+Shift+F8")]));
        // An invalid chord never reaches the daemon.
        let error = block_on(save_shortcut(&fake, &stored, "hotkey", "Ctrl")).unwrap_err();
        assert_eq!(error, "Use modifiers with one key, two modifiers, or a function key");
        assert_eq!(fake.writes().len(), 1);
        assert_eq!(shortcut_reset("hotkey"), "Ctrl+Win");
        assert_eq!(shortcut_reset("tts_hotkey_read_clone"), "");
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
    fn lists_keep_a_saved_value_they_do_not_offer() {
        let stored = settings(&[("paste_delay_ms", "75"), ("recording_limit_seconds", "300")]);
        let delays = preset_choices(DELAYS, &stored, "paste_delay_ms", "ms");
        assert_eq!(delays.labels[0], "75 ms");
        assert_eq!((delays.values[0].as_str(), delays.selected), ("75", 0));
        let limits = preset_choices(LIMITS, &stored, "recording_limit_seconds", "seconds");
        assert_eq!((limits.labels.len(), limits.labels[limits.selected].as_str()), (5, "5 minutes"));
    }

    #[test]
    fn model_list_keeps_the_saved_model_first_when_not_advertised() {
        let stored = settings(&[("transcription_model", "gpt-4o-mini-transcribe"), ("llm_model", "text-light")]);
        let mut models = Models::default();
        // Until a list arrives the saved model is the only, unmarked, choice.
        let before = models.choices(&models.transcription, &stored, "transcription_model");
        assert_eq!(before.labels, ["gpt-4o-mini-transcribe"]);
        let generation = models.begin();
        assert_eq!(models.status(true), "Checking the endpoint...");
        assert!(!models.receive(&json!({"generation": generation + 1, "transcription": ["x"]})));
        assert!(models.receive(&json!({
            "generation": generation, "transcription": ["canary-1b-v2", "whisper-1"], "chat": ["gpt-4.1-nano", "text-light"]
        })));
        let list = models.choices(&models.transcription, &stored, "transcription_model");
        assert_eq!(list.labels, ["gpt-4o-mini-transcribe (not advertised)", "canary-1b-v2", "whisper-1"]);
        assert_eq!((list.values[2].as_str(), list.selected), ("whisper-1", 0));
        let chat = models.choices(&models.chat, &stored, "llm_model");
        assert_eq!((chat.labels.len(), chat.selected), (2, 1));
        assert_eq!(models.status(true), "Connected: 2 transcription and 2 chat models available.");
        // A failed refresh keeps the lists.
        let generation = models.begin();
        models.receive(&json!({"generation": generation, "error": "unauthorized"}));
        assert_eq!(models.status(true), "Could not list models: unauthorized");
        assert_eq!(models.transcription.len(), 2);
        assert_eq!(models.status(false), "Add an API key to load the available models.");
    }

    #[test]
    fn endpoint_and_key() {
        let fake = Fake::default();
        assert_eq!(block_on(save_key(&fake, "  ")).unwrap_err(), "Paste an API key before saving.");
        assert_eq!(block_on(save_key(&fake, " sk-1 ")).unwrap(), KEY_SAVED);
        assert_eq!(*fake.keys.borrow(), ["sk-1"]);
        let stored = settings(&[("api_base_url", "http://localhost:1234/v1")]);
        assert_eq!(endpoint_change(&stored, " http://localhost:1234/v1 "), None);
        assert_eq!(endpoint_change(&stored, "  "), None);
        assert_eq!(endpoint_change(&stored, ENDPOINTS[1]).as_deref(), Some(ENDPOINTS[1]));
        assert_eq!(endpoint_presets(&stored)[0], "http://localhost:1234/v1");
        assert_eq!(endpoint_presets(&settings(&[("api_base_url", ENDPOINTS[0])])).len(), 2);
    }

    #[test]
    fn languages_toggle_and_summarize() {
        let auto = settings(&[("languages", r#"["auto"]"#)]);
        assert!(auto_detect(&auto));
        assert_eq!(languages_summary(&auto), "Auto-detect");
        assert_eq!(selected_languages(&languages(&auto), &["en", "fr"], &["fr"]), ["fr"]);
        let two = settings(&[("languages", r#"["fr","en"]"#)]);
        assert_eq!(languages_summary(&two), "French, English");
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
        let gone = microphone_choices(&[], &settings(&[("microphone_device", "USB")]));
        assert_eq!(gone.labels, ["USB (not connected)", "Auto-detect"]);
    }

    fn catalog() -> Value {
        json!([
            {"id":"qwen3-tts","name":"Qwen","capabilities":{
                "modes":{"speaker":["worker"]},"voice_fields":{"speaker":["voice","voice_instruction"]},
                "voices":[{"id":18,"name":"Test speaker"}],
                "engine_options":{"temperature":"number","num_step":"integer","stream":"boolean"}
            }},
            {"id":"omnivoice","name":"Omni","capabilities":{
                "modes":{"auto":["worker"]},"voice_fields":{"auto":[]},"engine_options":{}
            }}
        ])
    }

    #[test]
    fn speech_catalog_selects_a_default_and_drops_stale_answers() {
        let mut list = Catalog::default();
        let generation = list.begin_refresh();
        let view = speech_view(&list, &Settings::new(), &Settings::new(), true);
        assert_eq!(view.model_labels, ["Loading models..."]);
        assert!(!view.model_enabled && !view.refresh_enabled);
        assert_eq!(view.refresh_label, "Loading...");
        assert!(list.receive(&json!({"generation": generation + 1, "models": []})).is_none());
        assert_eq!(list.receive(&json!({"generation": generation, "models": catalog()})), Some(Ok(())));

        let fake = Fake::default();
        let status = block_on(adopt_catalog(&fake, &list.models, &Settings::new()));
        assert_eq!(status.as_deref(), Some("Selected speech model qwen3-tts."));
        assert_eq!(fake.writes(), pairs(&[("tts_voice:qwen3-tts", "18"), ("tts_model", "qwen3-tts")]));

        let stored = settings(&[
            ("tts_model", "qwen3-tts"),
            ("tts_voice:qwen3-tts", "18"),
            ("tts_engine_options", r#"{"qwen3-tts":{"num_step":8,"stream":false}}"#),
        ]);
        assert_eq!(block_on(adopt_catalog(&fake, &list.models, &stored)), None);
        let drafts = settings(&[("engine_draft:qwen3-tts:temperature", "0.5")]);
        let view = speech_view(&list, &stored, &drafts, true);
        assert_eq!(view.model_labels, ["Choose a model", "Qwen (qwen3-tts)", "Omni (omnivoice)"]);
        assert_eq!((view.model_index, view.voice_index), (1, 1));
        assert_eq!(view.voice_labels, ["Engine default", "Test speaker (18)"]);
        assert!(view.voice_enabled && view.instruction_enabled);
        let values: Vec<_> = view.engine.iter().map(|f| (f.name.as_str(), f.value.as_str())).collect();
        assert_eq!(values, [("num_step", "8"), ("stream", "false"), ("temperature", "0.5")]);
        assert_eq!(model_at(&list, 2), "omnivoice");
        assert_eq!(model_at(&list, 0), "");
        assert_eq!(voice_at(&list.models[0], 0), "");

        list.begin_refresh();
        let generation = list.generation;
        assert_eq!(
            list.receive(&json!({"generation": generation, "error": "unauthorized"})),
            Some(Err("Speech models: unauthorized".into()))
        );
        let view = speech_view(&list, &stored, &Settings::new(), true);
        assert_eq!(view.model_labels, ["qwen3-tts (not advertised)"]);
        assert!(!view.voice_enabled && view.engine.is_empty());
    }

    #[test]
    fn engine_options_are_typed_per_field() {
        let models: Vec<SpeechModel> = serde_json::from_value(catalog()).unwrap();
        let field = |n: &str, k: &str, v: &str| (n.to_string(), k.to_string(), v.to_string());
        let typed = [field("num_step", "integer", "1.5"), field("stream", "boolean", ""), field("temperature", "number", "")];
        assert_eq!(engine_options(&models[0], &typed, "{}").unwrap_err(), "num_step must be integer.");
        let typed = [field("num_step", "integer", "8"), field("stream", "boolean", "false"), field("temperature", "number", "0.8")];
        assert_eq!(
            engine_options(&models[0], &typed, r#"{"omnivoice":{"x":1}}"#).unwrap().unwrap(),
            r#"{"omnivoice":{"x":1},"qwen3-tts":{"num_step":8,"stream":false,"temperature":0.8}}"#
        );
        let saved = r#"{"qwen3-tts":{"num_step":8,"stream":false,"temperature":0.8}}"#;
        assert_eq!(engine_options(&models[0], &typed, saved).unwrap(), None);
        let empty = [field("num_step", "integer", " ")];
        assert_eq!(engine_options(&models[0], &empty, saved).unwrap().unwrap(), "{}");
        assert_eq!((boolean_index("false"), boolean_at(2), boolean_at(0)), (2, "false", ""));

        let stored = settings(&[("tts_voice_instruction:qwen3-tts", "Softly")]);
        assert!(instruction_change(&stored, "qwen3-tts", &"x".repeat(1025)).unwrap_err().contains("1,024"));
        assert_eq!(instruction_change(&stored, "qwen3-tts", "Softly"), Ok(None));
        assert_eq!(instruction_change(&stored, "qwen3-tts", "").unwrap().as_deref(), Some("tts_voice_instruction:qwen3-tts"));
    }

    #[test]
    fn events_become_status_text() {
        use dictto_core::events;
        assert_eq!(event_status(events::SPEECH_ERROR, &json!({"message":"No voice"})).unwrap(), "No voice");
        assert_eq!(event_status(events::RECORDING_STATE_CHANGED, &json!({"state":"recording"})).unwrap(), "recording");
        assert_eq!(event_status(events::WAVEFORM_DATA, &json!({})), None);
    }
}
