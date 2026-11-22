use super::*;

pub(super) const ENDPOINT: usize = 201;
pub(super) const TRANSCRIPTION_MODEL: usize = 202;
pub(super) const CLEANUP_MODEL: usize = 203;
pub(super) const REFRESH: usize = 206;
pub(super) const KEY: usize = 210;
const CONNECTION_STATUS: usize = 212;
pub(super) const SAVE_KEY: usize = 310;
const REMOVE_KEY: usize = 311;
const ENDPOINTS: &[&str] = &["https://api.openai.com/v1", "https://api.groq.com/openai/v1"];

/// Models the endpoint advertised, loaded once per endpoint and key and kept until Refresh.
#[derive(Default)]
pub(super) struct Models {
    transcription: Vec<String>,
    chat: Vec<String>,
    generation: u64,
    loading: bool,
    loaded: bool,
    error: Option<String>,
}

impl Models {
    /// A reply still in flight belongs to the old endpoint or key.
    pub(super) fn forget(&mut self) {
        *self = Self { generation: self.generation + 1, ..Default::default() };
    }
}

impl App {
    fn load_models(&mut self, refresh: bool) {
        self.models.generation += 1;
        self.models.loading = true;
        self.models.error = None;
        self.runtime.discover_models(self.models.generation, refresh);
    }

    pub(super) fn autoload_models(&mut self) {
        if !self.models.loaded && !self.models.loading {
            self.load_models(false);
        }
    }

    /// Model lists keep the saved model first when the endpoint does not advertise it, so
    /// loading a catalog never switches models by itself.
    unsafe fn model_combo(&mut self, id: usize, key: &str, models: Vec<String>, rect: [i32; 4]) {
        let missing = if self.models.loaded { "(not advertised)" } else { "" };
        let options = models.into_iter().map(|m| (m.clone(), m)).collect();
        self.bound_combo(id, key, options, missing, rect);
    }

    pub(super) unsafe fn render_transcription_model(&mut self, y: i32) {
        self.label("Transcription model", [20, y + 4, 210, 22]);
        self.model_combo(TRANSCRIPTION_MODEL, "transcription_model", self.models.transcription.clone(), [240, y, 290, 240]);
        let loading = self.models.loading;
        self.button(REFRESH, if loading { "Loading..." } else { "Refresh" }, [540, y - 1, 94, 30]);
        let _ = EnableWindow(self.control(REFRESH), !loading && self.runtime.has_key());
    }

    pub(super) unsafe fn render_cleanup_model(&mut self, y: i32) {
        self.label("Cleanup model", [20, y + 4, 210, 22]);
        self.model_combo(CLEANUP_MODEL, "llm_model", self.models.chat.clone(), [240, y, 394, 240]);
    }

    fn connection_status(&self) -> String {
        if !self.runtime.has_key() {
            "Add an API key to load the available models.".into()
        } else if self.models.loading {
            "Checking the endpoint...".into()
        } else if let Some(error) = &self.models.error {
            format!("Could not list models: {error}")
        } else if self.models.loaded {
            format!(
                "Connected: {} transcription and {} chat models available.",
                self.models.transcription.len(),
                self.models.chat.len()
            )
        } else {
            String::new()
        }
    }

    pub(super) unsafe fn render_connection(&mut self) {
        self.label("Endpoint", [20, 70, 210, 22]);
        let saved = self.setting("api_base_url").to_string();
        let endpoint = self.add(
            ENDPOINT,
            w!("COMBOBOX"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32),
            [240, 66, 394, 200],
        );
        let mut urls: Vec<&str> = ENDPOINTS.to_vec();
        if !saved.is_empty() && !urls.contains(&saved.as_str()) {
            urls.insert(0, &saved);
        }
        for url in urls {
            SendMessageW(endpoint, CB_ADDSTRING, None, Some(LPARAM(wide(url).as_ptr() as isize)));
        }
        let _ = SetWindowTextW(endpoint, PCWSTR(wide(&saved).as_ptr()));
        self.hint("Any OpenAI-compatible base URL, including /v1. Pick one or type your own.", [240, 98, 394, 36]);
        self.label("API key", [20, 154, 210, 22]);
        let key = self.add(
            KEY,
            w!("EDIT"),
            "",
            WS_TABSTOP | WS_BORDER | WINDOW_STYLE((ES_AUTOHSCROLL | ES_PASSWORD) as u32),
            [240, 150, 210, 28],
        );
        let hint = self.runtime.key_hint().unwrap_or_else(|_| "Paste your API key".into());
        self.cue(key, &hint);
        self.button(SAVE_KEY, "Save", [458, 149, 84, 30]);
        self.button(REMOVE_KEY, "Remove", [550, 149, 84, 30]);
        let _ = EnableWindow(self.control(REMOVE_KEY), self.runtime.has_key());
        self.hint("Kept in Windows Credential Manager.", [240, 182, 394, 22]);
        self.label("Status", [20, 230, 210, 22]);
        let status = self.connection_status();
        self.add(CONNECTION_STATUS, w!("STATIC"), &status, WINDOW_STYLE::default(), [240, 230, 394, 44]);
    }

    unsafe fn save_endpoint(&mut self, url: &str) {
        let url = url.trim();
        if url.is_empty() || url == self.setting("api_base_url") {
            return;
        }
        if self.persist("api_base_url", url) {
            self.label_text(CONNECTION_STATUS, &self.connection_status());
        }
    }

    pub(super) unsafe fn api_command(&mut self, id: usize, notification: u32) -> bool {
        match (id, notification) {
            (REFRESH, BN_CLICKED) => {
                self.load_models(true);
                self.status = "Loading models from the endpoint...".into();
                self.render();
                let _ = SetFocus(Some(self.control(TRANSCRIPTION_MODEL)));
            }
            (ENDPOINT, CBN_SELCHANGE) => {
                let combo = self.control(ENDPOINT);
                let index = SendMessageW(combo, CB_GETCURSEL, None, None).0;
                let length = SendMessageW(combo, CB_GETLBTEXTLEN, Some(WPARAM(index as usize)), None).0;
                if index >= 0 && length > 0 {
                    let mut buffer = vec![0u16; length as usize + 1];
                    SendMessageW(combo, CB_GETLBTEXT, Some(WPARAM(index as usize)), Some(LPARAM(buffer.as_mut_ptr() as isize)));
                    self.save_endpoint(&String::from_utf16_lossy(&buffer[..length as usize]));
                }
            }
            (ENDPOINT, CBN_KILLFOCUS) if !self.control(ENDPOINT).0.is_null() => {
                self.save_endpoint(&self.text(ENDPOINT));
            }
            (SAVE_KEY, BN_CLICKED) => {
                let key = self.text(KEY);
                if key.trim().is_empty() {
                    self.status = "Paste an API key before saving.".into();
                } else {
                    self.status = match self.runtime.save_key(key.trim()) {
                        Ok(()) => {
                            self.forget_catalogs();
                            self.autoload();
                            self.render();
                            "API key saved.".into()
                        }
                        Err(e) => e,
                    };
                }
            }
            (REMOVE_KEY, BN_CLICKED) => {
                if MessageBoxW(
                    Some(self.window),
                    w!("Remove the saved API key? Dictation stops working until you add one."),
                    w!("Dictto"),
                    MB_YESNO | MB_ICONQUESTION,
                ) == IDYES
                {
                    self.status = match self.runtime.remove_key() {
                        Ok(()) => {
                            self.forget_catalogs();
                            self.render();
                            "API key removed.".into()
                        }
                        Err(e) => e,
                    };
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) unsafe fn receive_models(&mut self, payload: Value) {
        if payload["generation"].as_u64() != Some(self.models.generation) {
            return;
        }
        self.models.loading = false;
        if let Some(error) = payload["error"].as_str() {
            self.models.error = Some(error.into());
            self.status = format!("Models: {error}");
        } else {
            let list = |key: &str| serde_json::from_value(payload[key].clone()).unwrap_or_default();
            self.models.transcription = list("transcription");
            self.models.chat = list("chat");
            self.models.loaded = true;
            self.status = self.connection_status();
        }
        if self.window.0.is_null() {
            return;
        }
        match self.page {
            DICTATION => self.render(),
            // Only the status changes; a re-render would drop an endpoint being typed.
            CONNECTION => self.label_text(CONNECTION_STATUS, &self.connection_status()),
            _ => {}
        }
    }
}

#[cfg(test)]
impl App {
    pub(super) fn models_generation(&self) -> u64 {
        self.models.generation
    }
}
