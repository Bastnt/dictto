use super::*;

pub(super) const MODEL_COMBO: usize = 202;
pub(super) const REFRESH: usize = 206;

/// Transcription routes the endpoint advertised on the last refresh. Empty until the user
/// asks, so opening the API page never sends a request.
#[derive(Default)]
pub(super) struct TranscriptionCatalog {
    models: Vec<String>,
    generation: u64,
    loading: bool,
}

impl TranscriptionCatalog {
    /// Called when the endpoint changes or the window closes: a reply still in flight
    /// belongs to the old endpoint.
    pub(super) fn forget(&mut self) {
        self.models.clear();
        self.generation += 1;
        self.loading = false;
    }
}

impl App {
    pub(super) unsafe fn render_transcription_model(&mut self, rect: [i32; 4]) {
        let saved = self.drafts.get("transcription_model").cloned().unwrap_or_default();
        let catalog = &self.transcription;
        // The saved model stays first so a refresh never silently switches models.
        let mut labels = Vec::new();
        if !catalog.models.contains(&saved) {
            labels.push(if catalog.models.is_empty() || saved.is_empty() {
                saved.clone()
            } else {
                format!("{saved} (not advertised)")
            });
        }
        labels.extend(catalog.models.iter().cloned());
        let selected = catalog.models.iter().position(|m| *m == saved).unwrap_or(0);
        let [x, y, width, height] = rect;
        self.combo(MODEL_COMBO, &labels, selected, [x, y, width - 114, 240]);
        self.button(REFRESH, if self.transcription.loading { "Loading…" } else { "Refresh" },
            [x + width - 104, y, 104, height]);
        let _ = EnableWindow(self.control(REFRESH), !self.transcription.loading);
    }

    pub(super) unsafe fn api_command(&mut self, id: usize, notification: u32) -> bool {
        match (id, notification) {
            (MODEL_COMBO, CBN_SELCHANGE) => {
                let index = SendMessageW(self.control(MODEL_COMBO), CB_GETCURSEL, None, None).0 as usize;
                let saved = self.drafts.get("transcription_model").cloned().unwrap_or_default();
                let offset = usize::from(!self.transcription.models.contains(&saved));
                if let Some(model) = index.checked_sub(offset).and_then(|i| self.transcription.models.get(i)).cloned() {
                    self.persist("transcription_model", &model);
                }
            }
            (REFRESH, BN_CLICKED) => {
                self.capture_drafts();
                self.transcription.generation += 1;
                self.transcription.loading = true;
                self.status = "Loading transcription models from the configured API…".into();
                self.runtime.discover_transcription_models(self.transcription.generation);
                self.render();
            }
            _ => return false,
        }
        true
    }

    pub(super) unsafe fn receive_transcription_models(&mut self, payload: Value) {
        if payload["generation"].as_u64() != Some(self.transcription.generation) {
            return;
        }
        self.transcription.loading = false;
        self.capture_drafts();
        self.status = if let Some(error) = payload["error"].as_str() {
            format!("Transcription models: {error}")
        } else {
            self.transcription.models =
                serde_json::from_value(payload["models"].clone()).unwrap_or_default();
            match self.transcription.models.len() {
                0 => "This API advertises no transcription models.".into(),
                n => format!("{n} transcription models available."),
            }
        };
        if self.page == 2 && !self.window.0.is_null() {
            self.render();
        }
    }
}

#[cfg(test)]
impl App {
    pub(super) fn transcription_generation(&self) -> u64 {
        self.transcription.generation
    }
}
