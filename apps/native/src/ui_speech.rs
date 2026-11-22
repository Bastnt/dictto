use super::*;

const MODEL: usize = 610;
const VOICE_LIST: usize = 611;
const REFRESH: usize = 614;
const INSTRUCTION: usize = 620;
const AUDIO_FOLDER: usize = 624;
const VIRTUAL_MIC: usize = 625;
// One control per engine option, two per row.
const OPTION: usize = 650;
const MAX_OPTIONS: usize = 8;

impl App {
    fn selected_speech_model(&self) -> Option<&SpeechModel> {
        self.speech_models
            .iter()
            .find(|m| Some(&m.id) == self.drafts.get("tts_model"))
    }

    pub(super) fn refresh_speech_models(&mut self) {
        self.catalog_generation += 1;
        self.catalog_loading = true;
        self.runtime.discover_speech_models(self.catalog_generation);
    }

    pub(super) unsafe fn receive_speech_models(&mut self, payload: Value) {
        if payload["generation"].as_u64() != Some(self.catalog_generation) {
            return;
        }
        self.catalog_loading = false;
        self.capture_engine_draft();
        self.speech_models.clear();
        let message = if let Some(error) = payload["error"].as_str() {
            Some(format!("Speech models: {error}"))
        } else {
            match serde_json::from_value(payload["models"].clone()) {
                Ok(models) => {
                    self.speech_models = models;
                    if self.speech_models.is_empty() {
                        Some("This API advertises no speech models.".into())
                    } else {
                        self.choose_initial_speech_model()
                    }
                }
                Err(e) => Some(format!("Invalid speech catalog: {e}")),
            }
        };
        // Loading happens in the background on every open; only news is worth a status.
        if let Some(message) = message {
            self.status = message;
        }
        if self.page == VOICE && !self.window.0.is_null() {
            self.render();
        }
    }

    unsafe fn choose_initial_speech_model(&mut self) -> Option<String> {
        if self.drafts.get("tts_model").is_some_and(|id| !id.is_empty()) {
            return None;
        }
        let model = dictto_native::speech_api::default_speech_model(&self.speech_models)
            .expect("nonempty catalog");
        let model_id = model.id.clone();
        let speaker = dictto_native::speech_api::default_speech_voice(model);
        let voice_key = format!("tts_voice:{model_id}");
        let instruction_key = format!("tts_voice_instruction:{model_id}");
        if !self.drafts.contains_key(&voice_key) && !self.drafts.contains_key(&instruction_key) {
            if let Some(speaker) = speaker {
                if !self.persist(&voice_key, &speaker.to_string()) {
                    return Some(self.status.clone());
                }
            }
        }
        if !self.persist("tts_model", &model_id) {
            return Some(self.status.clone());
        }
        Some(format!("Selected speech model {model_id}."))
    }

    pub(super) unsafe fn render_speech(&mut self) {
        let model = self.selected_speech_model().cloned();
        self.label("Speech model", [20, 70, 210, 22]);
        let mut labels = vec![match self.drafts.get("tts_model").filter(|s| !s.is_empty()) {
            Some(id) if model.is_none() && !self.catalog_loading => format!("{id} (not advertised)"),
            _ if self.catalog_loading => "Loading models...".into(),
            _ if self.speech_models.is_empty() => "No models available".into(),
            _ => "Choose a model".into(),
        }];
        labels.extend(self.speech_models.iter().map(|m| format!("{} ({})", m.name, m.id)));
        let index = self
            .speech_models
            .iter()
            .position(|m| Some(&m.id) == self.drafts.get("tts_model"))
            .map_or(0, |i| i + 1);
        self.combo(MODEL, &labels, index, [240, 66, 290, 200]);
        let _ = EnableWindow(
            self.control(MODEL),
            !self.speech_models.is_empty() && !self.catalog_loading,
        );
        let loading = self.catalog_loading;
        self.button(REFRESH, if loading { "Loading..." } else { "Refresh" }, [540, 65, 94, 30]);
        let _ = EnableWindow(self.control(REFRESH), !loading && self.runtime.has_key());
        let fields = |field: &str| {
            model.as_ref().is_some_and(|m| {
                m.capabilities.voice_fields.values().flatten().any(|f| f == field)
            })
        };
        let mut voices = vec!["Engine default".to_string()];
        let mut selected = 0;
        if let Some(m) = &model {
            let saved = self
                .drafts
                .get(&format!("tts_voice:{}", m.id))
                .and_then(|s| s.parse::<i64>().ok());
            for v in &m.capabilities.voices {
                if saved == Some(v.id) {
                    selected = voices.len();
                }
                voices.push(format!("{} ({})", v.name, v.id));
            }
        }
        self.label("Voice", [20, 108, 210, 22]);
        self.combo(VOICE_LIST, &voices, selected, [240, 104, 394, 200]);
        let _ = EnableWindow(self.control(VOICE_LIST), fields("voice") && voices.len() > 1);
        self.label("Voice instruction", [20, 146, 210, 22]);
        let instruction = model
            .as_ref()
            .map(|m| self.setting(&format!("tts_voice_instruction:{}", m.id)).to_string())
            .unwrap_or_default();
        self.edit(
            INSTRUCTION,
            &instruction,
            "Optional, for example: calm, slow, warm",
            [240, 142, 394, 28],
        );
        let _ = EnableWindow(self.control(INSTRUCTION), fields("voice_instruction"));
        let y = self.render_engine_options(model.as_ref()) + 16;
        let mic_enabled = self.setting("tts_virtual_mic") == "true";
        self.check(VIRTUAL_MIC, "Send speech to VB-CABLE", mic_enabled, [20, y + 4, 440, 24]);
        self.button(AUDIO_FOLDER, "Open audio folder", [478, y, 156, 30]);
        self.hint(
            "Switches the Windows communications microphone to VB-CABLE while speaking, so calls hear the voice.",
            [20, y + 32, 614, 36],
        );
    }

    /// Returns the y just below the options.
    unsafe fn render_engine_options(&mut self, model: Option<&SpeechModel>) -> i32 {
        self.label("Engine options", [20, 192, 210, 22]);
        self.engine_fields = model
            .map(|m| {
                m.capabilities
                    .engine_options
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if self.engine_fields.is_empty() {
            self.hint("This model has no options.", [240, 192, 394, 22]);
            return 214;
        }
        let model_id = model.map_or("", |m| m.id.as_str()).to_string();
        let saved = self.saved_engine_options(&model_id);
        let fields: Vec<_> = self.engine_fields.iter().take(MAX_OPTIONS).cloned().collect();
        for (i, (name, kind)) in fields.into_iter().enumerate() {
            let x = 240 + (i % 2) as i32 * 200;
            let y = 188 + (i / 2) as i32 * 36;
            self.label(&name, [x, y + 4, 100, 22]);
            let value = self
                .drafts
                .get(&format!("engine_draft:{model_id}:{name}"))
                .cloned()
                .or_else(|| {
                    saved
                        .get(&name)
                        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                })
                .unwrap_or_default();
            if kind == "boolean" {
                let index = match value.as_str() {
                    "true" => 1,
                    "false" => 2,
                    _ => 0,
                };
                let labels = ["Default".to_string(), "On".into(), "Off".into()];
                self.combo(OPTION + i, &labels, index, [x + 100, y, 90, 120]);
            } else {
                self.edit(OPTION + i, &value, "Default", [x + 100, y, 90, 28]);
            }
        }
        let bottom = 188 + self.engine_fields.len().min(MAX_OPTIONS).div_ceil(2) as i32 * 36;
        let hidden = self.engine_fields.len().saturating_sub(MAX_OPTIONS);
        if hidden > 0 {
            self.hint(&format!("{hidden} more options are not shown."), [240, bottom, 394, 22]);
            return bottom + 22;
        }
        bottom
    }

    fn saved_engine_options(&self, model: &str) -> serde_json::Map<String, Value> {
        serde_json::from_str::<Value>(self.setting("tts_engine_options"))
            .ok()
            .and_then(|all| all.get(model).and_then(Value::as_object).cloned())
            .unwrap_or_default()
    }

    unsafe fn option_text(&self, index: usize, kind: &str) -> String {
        if kind == "boolean" {
            match SendMessageW(self.control(OPTION + index), CB_GETCURSEL, None, None).0 {
                1 => "true".into(),
                2 => "false".into(),
                _ => String::new(),
            }
        } else {
            self.text(OPTION + index).trim().to_string()
        }
    }

    /// Keeps typed text that a re-render would otherwise drop.
    pub(super) unsafe fn capture_engine_draft(&mut self) {
        if self.control(INSTRUCTION).0.is_null() {
            return;
        }
        let Some(model) = self.selected_speech_model().map(|m| m.id.clone()) else {
            return;
        };
        self.drafts
            .insert(format!("tts_voice_instruction:{model}"), self.text(INSTRUCTION));
        let fields: Vec<_> = self.engine_fields.iter().take(MAX_OPTIONS).cloned().collect();
        for (i, (name, kind)) in fields.iter().enumerate() {
            if !self.control(OPTION + i).0.is_null() {
                let value = self.option_text(i, kind);
                self.drafts.insert(format!("engine_draft:{model}:{name}"), value);
            }
        }
    }

    unsafe fn save_engine_options(&mut self) {
        let Some(model) = self.selected_speech_model().cloned() else {
            return;
        };
        let mut group = serde_json::Map::new();
        let fields: Vec<_> = self.engine_fields.iter().take(MAX_OPTIONS).cloned().collect();
        for (i, (name, kind)) in fields.iter().enumerate() {
            let text = self.option_text(i, kind);
            if text.is_empty() {
                continue;
            }
            let value = match kind.as_str() {
                "integer" => text.parse::<i64>().ok().map(Value::from),
                "number" => text
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number),
                "boolean" => Some(Value::Bool(text == "true")),
                _ => Some(Value::String(text)),
            };
            let Some(value) = value else {
                self.status = format!("{name} must be {kind}.");
                return;
            };
            group.insert(name.clone(), value);
        }
        let saved = self.setting("tts_engine_options").to_string();
        let saved = if saved.is_empty() { "{}".to_string() } else { saved };
        match engine_options(&model, group, &saved) {
            Ok(options) if options != saved => {
                self.persist("tts_engine_options", &options);
            }
            Ok(_) => {}
            Err(error) => self.status = error,
        }
    }

    pub(super) unsafe fn speech_command(&mut self, id: usize, notification: u32) -> bool {
        let option = (OPTION..OPTION + MAX_OPTIONS).contains(&id);
        match (id, notification) {
            (MODEL, CBN_SELCHANGE) => {
                self.capture_engine_draft();
                let index = SendMessageW(self.control(MODEL), CB_GETCURSEL, None, None).0 as usize;
                let model = index
                    .checked_sub(1)
                    .and_then(|i| self.speech_models.get(i))
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                self.persist("tts_model", &model);
                self.render();
            }
            (VOICE_LIST, CBN_SELCHANGE) => {
                let index =
                    SendMessageW(self.control(VOICE_LIST), CB_GETCURSEL, None, None).0 as usize;
                let voice = index
                    .checked_sub(1)
                    .and_then(|i| self.selected_speech_model()?.capabilities.voices.get(i))
                    .map(|v| v.id.to_string())
                    .unwrap_or_default();
                if let Some(model) = self.selected_speech_model() {
                    self.persist(&format!("tts_voice:{}", model.id), &voice);
                }
            }
            (REFRESH, BN_CLICKED) => {
                self.capture_engine_draft();
                self.refresh_speech_models();
                self.status = "Loading speech models...".into();
                self.render();
                let _ = SetFocus(Some(self.control(MODEL)));
            }
            (INSTRUCTION, EN_KILLFOCUS) => {
                if self.control(INSTRUCTION).0.is_null() {
                    return true;
                }
                let Some(key) = self
                    .selected_speech_model()
                    .map(|m| format!("tts_voice_instruction:{}", m.id))
                else {
                    return true;
                };
                let value = self.text(INSTRUCTION);
                if value.chars().count() > 1024 {
                    self.status = "Voice instructions are limited to 1,024 characters.".into();
                } else if self.runtime.get_setting(&key).unwrap_or_default() != value {
                    self.persist(&key, &value);
                }
            }
            (_, EN_KILLFOCUS) if option => {
                if !self.control(id).0.is_null() {
                    self.save_engine_options();
                }
            }
            (_, CBN_SELCHANGE) if option => self.save_engine_options(),
            (VIRTUAL_MIC, BN_CLICKED) => {
                let enabled =
                    SendMessageW(self.control(VIRTUAL_MIC), BM_GETCHECK, None, None).0 != 0;
                self.persist("tts_virtual_mic", if enabled { "true" } else { "false" });
            }
            (AUDIO_FOLDER, BN_CLICKED) => {
                use std::os::windows::ffi::OsStrExt;
                let folder = self
                    .runtime
                    .speech_audio_dir()
                    .expect("the native runtime is built with speech");
                if let Err(error) = std::fs::create_dir_all(&folder) {
                    self.status = format!("Could not create the audio folder: {error}");
                } else {
                    let path: Vec<u16> =
                        folder.as_os_str().encode_wide().chain(Some(0)).collect();
                    let result = ShellExecuteW(
                        Some(self.window),
                        w!("open"),
                        PCWSTR(path.as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );
                    self.status = if result.0 as isize > 32 {
                        "Opened audio folder.".into()
                    } else {
                        format!(
                            "Windows could not open the audio folder (error {}).",
                            result.0 as isize
                        )
                    };
                }
            }
            _ => return false,
        }
        true
    }
}

/// `group` replaces the model's saved options; an empty group removes them.
fn engine_options(
    model: &SpeechModel,
    group: serde_json::Map<String, Value>,
    saved: &str,
) -> Result<String, String> {
    if let Some(name) = group.keys().find(|name| !model.capabilities.engine_options.contains_key(*name)) {
        return Err(format!("Unknown {} option: {name}", model.id));
    }
    let mut groups: serde_json::Map<String, Value> =
        serde_json::from_str(saved).map_err(|e| format!("Invalid saved options: {e}"))?;
    if group.is_empty() {
        groups.remove(&model.id);
    } else {
        groups.insert(model.id.clone(), Value::Object(group));
    }
    Ok(Value::Object(groups).to_string())
}
