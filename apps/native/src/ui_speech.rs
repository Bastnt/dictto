use super::*;

pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("tts_hotkey_sample_mic", "Record voice sample (microphone)"),
    (
        "tts_hotkey_sample_loopback",
        "Record voice sample (Windows playback)",
    ),
    (
        "tts_hotkey_speech_to_speech",
        "Repeat my speech in the cloned voice",
    ),
    ("tts_hotkey_read_clone", "Read text in the cloned voice"),
    ("tts_hotkey_read_voice", "Read text in the engine voice"),
];

impl App {
    unsafe fn combo(&mut self, id: usize, labels: &[String], selected: usize, rect: [i32; 4]) {
        let hwnd = self.add(
            id,
            w!("COMBOBOX"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            rect,
        );
        for label in labels {
            SendMessageW(
                hwnd,
                CB_ADDSTRING,
                None,
                Some(LPARAM(wide(label).as_ptr() as isize)),
            );
        }
        SendMessageW(hwnd, CB_SETCURSEL, Some(WPARAM(selected)), None);
    }

    fn selected_speech_model(&self) -> Option<&SpeechModel> {
        self.speech_models
            .iter()
            .find(|m| Some(&m.id) == self.drafts.get("tts_model"))
    }

    pub(super) fn refresh_speech_models(&mut self) {
        self.catalog_generation += 1;
        self.catalog_loading = true;
        self.status = "Loading speech models from the configured API…".into();
        self.runtime.discover_speech_models(self.catalog_generation);
    }

    pub(super) unsafe fn receive_speech_models(&mut self, payload: Value) {
        if payload["generation"].as_u64() != Some(self.catalog_generation) {
            return;
        }
        self.catalog_loading = false;
        self.capture_drafts();
        self.speech_models.clear();
        self.status = if let Some(error) = payload["error"].as_str() {
            format!("Speech discovery: {error}")
        } else {
            match serde_json::from_value(payload["models"].clone()) {
                Ok(models) => {
                    self.speech_models = models;
                    if self.speech_models.is_empty() {
                        "This API advertises no speech models.".into()
                    } else {
                        self.choose_initial_speech_model()
                    }
                }
                Err(e) => format!("Invalid speech catalog: {e}"),
            }
        };
        if self.page == 4 && !self.window.0.is_null() {
            self.render();
        }
    }

    unsafe fn choose_initial_speech_model(&mut self) -> String {
        if self
            .drafts
            .get("tts_model")
            .is_some_and(|id| !id.is_empty())
        {
            return "Speech models loaded.".into();
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
                    return self.status.clone();
                }
            }
        }
        if !self.persist("tts_model", &model_id) {
            return self.status.clone();
        }
        format!("Selected {model_id}.")
    }

    pub(super) unsafe fn render_speech_shortcuts(&mut self) {
        self.label("Speech shortcuts", [20, 62, 614, 22]);
        self.label(
            "Enter a chord such as Ctrl+Alt+F8. Clear the field to disable it.",
            [20, 87, 614, 25],
        );
        for (i, (key, label)) in SHORTCUTS.iter().enumerate() {
            let y = 129 + 48 * i as i32;
            self.label(label, [20, y + 4, 315, 23]);
            self.edit(630 + i, key, [340, y, 197, 28]);
            self.button(640 + i, "Save", [550, y, 84, 28]);
        }
        self.label("Read text uses the selection in the focused app, then the clipboard if nothing is selected.", [20, 365, 614, 28]);
    }

    pub(super) unsafe fn render_speech(&mut self) {
        let model = self.selected_speech_model().cloned();
        self.label("Speech model", [20, 60, 140, 22]);
        let mut labels = vec![
            match self.drafts.get("tts_model").filter(|s| !s.is_empty()) {
                Some(id) if model.is_none() => format!("{id} (not discovered)"),
                _ if self.catalog_loading => "Loading models…".into(),
                _ if self.speech_models.is_empty() => "No models available".into(),
                _ => "Choose a model".into(),
            },
        ];
        labels.extend(
            self.speech_models
                .iter()
                .map(|m| format!("{} ({})", m.name, m.id)),
        );
        let index = self
            .speech_models
            .iter()
            .position(|m| Some(&m.id) == self.drafts.get("tts_model"))
            .map_or(0, |i| i + 1);
        self.combo(610, &labels, index, [168, 56, 357, 200]);
        let _ = EnableWindow(
            self.control(610),
            !self.speech_models.is_empty() && !self.catalog_loading,
        );
        self.button(614, "Refresh", [536, 55, 98, 30]);
        let _ = EnableWindow(self.control(614), !self.catalog_loading);
        let fields = |field: &str| {
            model.as_ref().is_some_and(|m| {
                m.capabilities
                    .voice_fields
                    .values()
                    .flatten()
                    .any(|f| f == field)
            })
        };
        let mut voices = vec!["No numbered speaker".to_string()];
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
                voices.push(format!("{} · {}", v.id, v.name));
            }
        }
        self.label("Engine voice", [20, 103, 140, 22]);
        self.combo(611, &voices, selected, [168, 99, 466, 200]);
        let _ = EnableWindow(self.control(611), fields("voice") && voices.len() > 1);
        self.label("Voice instruction", [20, 145, 140, 22]);
        let instruction_key = format!(
            "tts_voice_instruction:{}",
            model.as_ref().map_or("", |m| m.id.as_str())
        );
        self.edit(620, &instruction_key, [168, 141, 357, 28]);
        self.button(621, "Save", [536, 140, 98, 30]);
        let instruction = fields("voice_instruction");
        let _ = EnableWindow(self.control(620), instruction);
        let _ = EnableWindow(self.control(621), instruction);
        let modes = model
            .as_ref()
            .map(|m| {
                m.capabilities
                    .modes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        self.label(&format!("Available modes: {modes}"), [20, 183, 614, 22]);
        self.label(
            "Clone actions use your last recorded voice sample.",
            [20, 213, 438, 26],
        );
        self.button(624, "Open audio folder", [478, 205, 156, 30]);
        self.label("Advanced engine options (JSON object)", [20, 253, 600, 22]);
        let key = format!(
            "engine_draft:{}",
            model.as_ref().map_or("", |m| m.id.as_str())
        );
        if let Some(m) = &model {
            let all = self
                .drafts
                .get("tts_engine_options")
                .and_then(|s| serde_json::from_str::<Value>(s).ok());
            self.drafts.entry(key.clone()).or_insert_with(|| {
                all.as_ref()
                    .and_then(|v| v.get(&m.id))
                    .map(Value::to_string)
                    .unwrap_or_else(|| "{}".into())
            });
        }
        self.edit(622, &key, [20, 281, 505, 28]);
        self.button(623, "Save", [536, 280, 98, 30]);
        let schema = model.as_ref().map(|m| &m.capabilities.engine_options);
        let enabled = schema.is_some_and(|s| !s.is_empty());
        let _ = EnableWindow(self.control(622), enabled);
        let _ = EnableWindow(self.control(623), enabled);
        let hint = schema
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k} ({v})"))
            .collect::<Vec<_>>()
            .join(", ");
        self.label(&hint, [20, 313, 614, 31]);
        let mic_enabled = self.drafts.get("tts_virtual_mic").is_some_and(|v| v == "true");
        self.check(625, "Send speech to VB-CABLE", mic_enabled, [20, 348, 614, 24]);
        self.label("Temporarily switches the Windows communications microphone during speech.", [20, 376, 614, 21]);
    }

    pub(super) unsafe fn capture_engine_draft(&mut self) {
        if !self.control(622).0.is_null() {
            if let Some(model) = self.selected_speech_model().map(|m| m.id.clone()) {
                self.drafts
                    .insert(format!("tts_voice_instruction:{model}"), self.text(620));
                self.drafts
                    .insert(format!("engine_draft:{model}"), self.text(622));
            }
        }
    }

    pub(super) unsafe fn speech_command(&mut self, id: usize, notification: u32) -> bool {
        match (id, notification) {
            (610, CBN_SELCHANGE) => {
                self.capture_drafts();
                let index = SendMessageW(self.control(610), CB_GETCURSEL, None, None).0 as usize;
                let model = index
                    .checked_sub(1)
                    .and_then(|i| self.speech_models.get(i))
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                self.persist("tts_model", &model);
                self.render();
            }
            (611, CBN_SELCHANGE) => {
                let index = SendMessageW(self.control(611), CB_GETCURSEL, None, None).0 as usize;
                let voice = index
                    .checked_sub(1)
                    .and_then(|i| self.selected_speech_model()?.capabilities.voices.get(i))
                    .map(|v| v.id.to_string())
                    .unwrap_or_default();
                if let Some(model) = self.selected_speech_model() {
                    self.persist(&format!("tts_voice:{}", model.id), &voice);
                }
            }
            (614, BN_CLICKED) => {
                self.capture_drafts();
                self.refresh_speech_models();
                self.render();
            }
            (621, BN_CLICKED) => {
                let value = self.text(620);
                if value.chars().count() > 1024 {
                    self.status = "Voice instructions are limited to 1,024 characters.".into();
                } else if let Some(model) = self.selected_speech_model() {
                    self.persist(&format!("tts_voice_instruction:{}", model.id), &value);
                }
            }
            (623, BN_CLICKED) => {
                let Some(model) = self.selected_speech_model().cloned() else {
                    return true;
                };
                match engine_options(
                    &model,
                    &self.text(622),
                    self.drafts
                        .get("tts_engine_options")
                        .map_or("{}", String::as_str),
                ) {
                    Ok(options) => {
                        self.persist("tts_engine_options", &options);
                    }
                    Err(error) => self.status = error,
                }
            }
            (625, BN_CLICKED) => {
                let enabled = SendMessageW(self.control(625), BM_GETCHECK, None, None).0 != 0;
                self.persist("tts_virtual_mic", if enabled { "true" } else { "false" });
            }
            (624, BN_CLICKED) => {
                use std::os::windows::ffi::OsStrExt;
                let folder = self.runtime.speech_audio_dir().unwrap_or_default();
                if let Err(error) = std::fs::create_dir_all(&folder) {
                    self.status = format!("Could not create the audio folder: {error}");
                } else {
                    let path: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
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
            (640..=644, BN_CLICKED) => {
                let value = self.text(id - 10);
                self.persist(SHORTCUTS[id - 640].0, value.trim());
            }
            _ => return false,
        }
        true
    }
}

fn engine_options(model: &SpeechModel, draft: &str, saved: &str) -> Result<String, String> {
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
