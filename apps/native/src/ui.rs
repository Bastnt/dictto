use dictto_native::runtime::NativeRuntime;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use windows_reactor::*;
#[path = "ui_models.rs"]
mod models;

#[derive(Clone)]
struct Input {
    runtime: NativeRuntime,
    events: Arc<Mutex<mpsc::Receiver<(String, Value)>>>,
    startup_error: String,
    smoke: bool,
}
impl PartialEq for Input {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.events, &other.events)
    }
}
#[derive(Clone)]
enum Msg {
    Failure(String),
    Page(usize),
    Toggle(&'static str, bool),
    Draft(&'static str, String),
    Save(&'static str),
    SaveModels,
    EditHotkey(bool),
    EditKey(bool),
    Advanced(bool),
    Password(String),
    SaveKey,
    RemoveKey,
    RefreshMics,
    Microphone(Option<usize>),
    Language(String, bool),
    Event(Option<(String, Value)>),
    Start,
    Stop,
    Cancel,
}
struct Settings {
    input: Input,
    settings: HashMap<String, String>,
    drafts: HashMap<String, String>,
    page: usize,
    password: String,
    key_hint: String,
    microphones: Vec<(String, String)>,
    status: String,
    error: String,
    hotkey_edit: bool,
    key_edit: bool,
    advanced: bool,
}
impl Settings {
    fn listen(&self, context: &ComponentContext<Self>) {
        let events = self.input.events.clone();
        context.spawn_background(move |cancel| loop {
            if cancel.is_cancelled() {
                return Msg::Event(None);
            }
            match events.lock().unwrap().recv_timeout(Duration::from_secs(1)) {
                Ok(event) => return Msg::Event(Some(event)),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Msg::Event(None),
                Err(_) => {}
            }
        });
    }
    fn persist(&mut self, key: &str, value: &str) {
        if matches!(key, "recording_limit_seconds" | "paste_delay_ms")
            && (value.parse::<u64>().is_err() || (key == "recording_limit_seconds" && value == "0"))
        {
            self.error = "Enter a valid whole number (recording limit must be positive)".into();
            return;
        }
        match self.input.runtime.set_setting(key, value) {
            Ok(()) => {
                self.settings.insert(key.into(), value.into());
                self.error.clear();
                self.status = "Saved".into();
            }
            Err(error) => self.error = error,
        }
    }
    fn refresh_mics(&mut self) {
        match self.input.runtime.microphones() {
            Ok(items) => {
                self.microphones = vec![(
                    "auto-detect".into(),
                    format!("Auto-detect ({})", self.input.runtime.current_microphone()),
                )];
                self.microphones
                    .extend(items.into_iter().map(|item| (item.name.clone(), item.name)));
            }
            Err(error) => self.error = error,
        }
    }
    fn switch(
        &self,
        context: &ViewContext<Self>,
        key: &'static str,
        label: &str,
        description: &str,
    ) -> View {
        row(
            caption(label, description),
            ToggleSwitch::new()
                .automation_name(label)
                .is_on(self.settings.get(key).map(String::as_str) == Some("true"))
                .on_toggled(context.callback(move |value| Msg::Toggle(key, value)))
                .into(),
        )
    }
    fn field(&self, context: &ViewContext<Self>, key: &'static str, label: &str) -> View {
        let enabled = key != "llm_model"
            || self.settings.get("cleanup_enabled").map(String::as_str) == Some("true");
        row(
            caption(label, ""),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(6.0)
                .children((
                    TextBox::new()
                        .width(210.0)
                        .automation_name(label)
                        .is_enabled(enabled)
                        .text(self.drafts.get(key).cloned().unwrap_or_default())
                        .on_text_changed(context.callback(move |value| Msg::Draft(key, value))),
                    Button::new()
                        .is_enabled(enabled)
                        .on_click(context.message(Msg::Save(key)))
                        .content("Save"),
                )),
        )
    }
    fn model_input(
        &self,
        context: &ViewContext<Self>,
        key: &'static str,
        label: &str,
        enabled: bool,
    ) -> View {
        TextBox::new()
            .automation_name(label)
            .is_enabled(enabled)
            .text(self.drafts.get(key).cloned().unwrap_or_default())
            .on_text_changed(context.callback(move |value| Msg::Draft(key, value)))
            .into()
    }
    fn model_group(
        &self,
        context: &ViewContext<Self>,
        key: &'static str,
        label: &str,
        toggle: &'static str,
        toggle_label: &str,
        description: &str,
    ) -> View {
        let heading = row(
            TextBlock::new().text(label).into(),
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(8.0)
                .children((
                    TextBlock::new()
                        .text(toggle_label)
                        .font_size(12.0)
                        .vertical_alignment(VerticalAlignment::Center),
                    ToggleSwitch::new()
                        .automation_name(toggle_label)
                        .is_on(self.settings.get(toggle).map(String::as_str) == Some("true"))
                        .on_toggled(context.callback(move |value| Msg::Toggle(toggle, value))),
                )),
        );
        StackPanel::new().spacing(5.0).children((
            heading,
            self.model_input(
                context,
                key,
                label,
                key != "llm_model"
                    || self.settings.get("cleanup_enabled").map(String::as_str) == Some("true"),
            ),
            TextBlock::new()
                .text(description)
                .font_size(12.0)
                .text_wrapping(TextWrapping::Wrap),
        ))
    }
    fn language_summary(&self) -> String {
        let values: Vec<String> = serde_json::from_str(
            self.settings
                .get("languages")
                .map(String::as_str)
                .unwrap_or("[]"),
        )
        .unwrap_or_default();
        if values.contains(&"auto".to_string()) || values.is_empty() {
            return "Auto-detect".into();
        }
        values
            .iter()
            .map(|code| {
                models::LANGUAGES
                    .iter()
                    .find(|(id, _)| id == code)
                    .map(|(_, name)| *name)
                    .unwrap_or(code)
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}
impl Component for Settings {
    type Input = Input;
    type Message = Msg;
    fn create(input: &Input, context: &ComponentContext<Self>) -> Self {
        let (settings, error) = match input.runtime.get_settings() {
            Ok(settings) => (settings, input.startup_error.clone()),
            Err(error) => (HashMap::new(), error),
        };
        let mut ui = Self {
            input: input.clone(),
            drafts: settings.clone(),
            settings,
            page: 0,
            password: String::new(),
            key_hint: input
                .runtime
                .key_hint()
                .unwrap_or_else(|_| "No API key saved".into()),
            microphones: vec![("auto-detect".into(), "Auto-detect".into())],
            status: if input.smoke {
                "Smoke test: recording services are disabled".into()
            } else {
                "Ready".into()
            },
            error,
            hotkey_edit: false,
            key_edit: false,
            advanced: false,
        };
        if !input.smoke {
            ui.refresh_mics();
        }
        ui.listen(context);
        ui
    }
    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        match message {
            Msg::Failure(error) => self.error = error,
            Msg::EditHotkey(active) => {
                self.hotkey_edit = active;
                if !active {
                    self.drafts.insert(
                        "hotkey".into(),
                        self.settings.get("hotkey").cloned().unwrap_or_default(),
                    );
                }
            }
            Msg::EditKey(active) => {
                self.key_edit = active;
                self.password.clear();
            }
            Msg::Advanced(active) => self.advanced = active,
            Msg::SaveModels => {
                for key in ["api_base_url", "transcription_model", "llm_model"] {
                    if key == "llm_model"
                        && self.settings.get("cleanup_enabled").map(String::as_str) != Some("true")
                    {
                        continue;
                    }
                    let value = self.drafts.get(key).cloned().unwrap_or_default();
                    if value.trim().is_empty() {
                        self.error = "Model settings cannot be empty".into();
                        break;
                    }
                    self.persist(key, value.trim());
                    if !self.error.is_empty() {
                        break;
                    }
                }
            }
            Msg::Page(page) => self.page = page,
            Msg::Toggle(key, value) => self.persist(key, if value { "true" } else { "false" }),
            Msg::Draft(key, value) => {
                self.drafts.insert(key.into(), value);
            }
            Msg::Save(key) => {
                let value = self.drafts.get(key).cloned().unwrap_or_default();
                if value.trim().is_empty() {
                    self.error = "Enter a value before saving".into();
                } else {
                    self.persist(key, value.trim());
                    if key == "hotkey" && self.error.is_empty() {
                        self.hotkey_edit = false;
                    }
                }
            }
            Msg::Password(value) => self.password = value,
            Msg::SaveKey => {
                if self.password.trim().is_empty() {
                    self.error = "Enter an API key before saving".into();
                } else {
                    match self.input.runtime.save_key(self.password.trim()) {
                        Ok(()) => {
                            self.password.clear();
                            self.key_edit = false;
                            self.key_hint = self
                                .input
                                .runtime
                                .key_hint()
                                .unwrap_or_else(|_| "API key saved".into());
                            self.error.clear();
                            self.status = "API key saved in Windows Credential Manager".into();
                        }
                        Err(error) => self.error = error,
                    }
                }
            }
            Msg::RemoveKey => {
                if confirm_remove() {
                    match self.input.runtime.remove_key() {
                        Ok(()) => {
                            self.key_hint = "No API key saved".into();
                            self.error.clear();
                            self.status = "API key removed".into();
                        }
                        Err(error) => self.error = error,
                    }
                }
            }
            Msg::RefreshMics => self.refresh_mics(),
            Msg::Microphone(Some(index)) => {
                if let Some((value, _)) = self.microphones.get(index).cloned() {
                    self.persist("microphone_device", &value);
                }
            }
            Msg::Microphone(None) => {}
            Msg::Language(code, enabled) => {
                let mut values: Vec<String> = serde_json::from_str(
                    self.settings
                        .get("languages")
                        .map(String::as_str)
                        .unwrap_or("[]"),
                )
                .unwrap_or_default();
                values.retain(|item| item != &code && item != "auto");
                if enabled {
                    if code == "auto" {
                        values.clear();
                    }
                    values.push(code);
                }
                self.persist("languages", &serde_json::to_string(&values).unwrap());
            }
            Msg::Event(Some((name, payload))) => {
                if name == "error" || name == "recording-error" || name == "dictation-error" {
                    self.error = payload["message"]
                        .as_str()
                        .or_else(|| payload.as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| payload.to_string());
                } else if name == "setting-changed" {
                    if let (Some(key), Some(value)) =
                        (payload["key"].as_str(), payload["value"].as_str())
                    {
                        self.settings.insert(key.into(), value.into());
                    }
                } else if name == "audio-devices-changed" {
                    self.refresh_mics();
                } else if name == "recording-state"
                    || name == "recording-state-changed"
                    || name == "processing-state"
                {
                    self.status = payload["state"]
                        .as_str()
                        .or_else(|| payload.as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| payload.to_string());
                }
                self.listen(context);
            }
            Msg::Event(None) => {}
            Msg::Start => self.input.runtime.start_recording(),
            Msg::Stop => self.input.runtime.stop_recording(),
            Msg::Cancel => self.input.runtime.cancel_recording(),
        }
    }
    fn view(&self, _input: &Input, context: &mut ViewContext<Self>) -> View {
        context.window_title("Dictto Native");
        let mut visuals = WindowVisuals::new()
            .client_size(700.0, 500.0)
            .theme(WindowTheme::System);
        if let Some(icon) = icon_path() {
            visuals = visuals.icon(icon);
        }
        context.window_visuals(visuals);
        let sender = context.sender();
        context.use_effect("tray", (), move || {
            if let Err(error) = install_tray() {
                release_startup_hook();
                let _ = sender.send(Msg::Failure(format!("Tray unavailable: {error}")));
            }
            Some(Box::new(remove_tray))
        });
        let nav = StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(6.0)
            .children((
                Button::new()
                    .on_click(context.message(Msg::Page(0)))
                    .content("General"),
                Button::new()
                    .on_click(context.message(Msg::Page(1)))
                    .content("Audio"),
                Button::new()
                    .on_click(context.message(Msg::Page(2)))
                    .content("API"),
            ));
        let mut content: Vec<View> = Vec::new();
        match self.page {
            0 => {
                content.push(self.switch(
                    context,
                    "auto_start",
                    "Start on boot",
                    "Launch Dictto when you sign in",
                ));
                content.push(self.switch(
                    context,
                    "sound_effects_enabled",
                    "Sound effects",
                    "Play a sound when recording starts and stops",
                ));
                content.push(row(
                    caption("Languages", "The languages you speak"),
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(10.0)
                        .children((
                            TextBlock::new()
                                .max_width(235.0)
                                .text(self.language_summary())
                                .text_trimming(TextTrimming::CharacterEllipsis)
                                .vertical_alignment(VerticalAlignment::Center),
                            Button::new()
                                .automation_name("Change languages")
                                .on_click(context.message(Msg::Page(3)))
                                .content("Change"),
                        )),
                ));
                if self.hotkey_edit {
                    content.push(self.field(context, "hotkey", "Keyboard shortcut"));
                    content.push(row(
                        TextBlock::new()
                            .text("Use Ctrl+Win or Ctrl+Shift+D")
                            .font_size(12.0)
                            .into(),
                        Button::new()
                            .on_click(context.message(Msg::EditHotkey(false)))
                            .content("Cancel"),
                    ));
                } else {
                    content.push(row(
                        caption(
                            "Hotkey",
                            "Hold to dictate, double-press for continuous recording",
                        ),
                        StackPanel::new()
                            .orientation(Orientation::Horizontal)
                            .spacing(10.0)
                            .children((
                                TextBlock::new()
                                    .text(self.settings.get("hotkey").cloned().unwrap_or_default())
                                    .vertical_alignment(VerticalAlignment::Center),
                                Button::new()
                                    .automation_name("Change hotkey")
                                    .on_click(context.message(Msg::EditHotkey(true)))
                                    .content("Change"),
                            )),
                    ));
                }
            }
            1 => {
                content.push(row(
                    caption("Microphone", "Choose the input device for recording"),
                    ComboBox::new()
                        .width(310.0)
                        .automation_name("Microphone")
                        .items_source(self.microphones.iter().map(|(_, label)| label.clone()))
                        .selected_index(
                            self.microphones.iter().position(|(id, _)| {
                                Some(id) == self.settings.get("microphone_device")
                            }),
                        )
                        .on_selection_changed(context.callback(Msg::Microphone))
                        .into(),
                ));
                content.push(row(
                    TextBlock::new()
                        .text(format!(
                            "Current input: {}",
                            self.input.runtime.current_microphone()
                        ))
                        .font_size(12.0)
                        .text_wrapping(TextWrapping::Wrap)
                        .into(),
                    Button::new()
                        .on_click(context.message(Msg::RefreshMics))
                        .content("Refresh"),
                ));
                content.push(row(
                    caption("Recording", "Try the selected microphone"),
                    StackPanel::new()
                        .orientation(Orientation::Horizontal)
                        .spacing(6.0)
                        .children((
                            Button::new()
                                .is_enabled(!self.input.smoke)
                                .on_click(context.message(Msg::Start))
                                .content("Record"),
                            Button::new()
                                .is_enabled(!self.input.smoke)
                                .on_click(context.message(Msg::Stop))
                                .content("Stop"),
                            Button::new()
                                .is_enabled(!self.input.smoke)
                                .on_click(context.message(Msg::Cancel))
                                .content("Cancel"),
                        )),
                ));
                content.push(
                    Button::new()
                        .on_click(context.message(Msg::Advanced(!self.advanced)))
                        .content(if self.advanced {
                            "Hide advanced settings"
                        } else {
                            "Advanced settings"
                        }),
                );
                if self.advanced {
                    content.push(self.field(
                        context,
                        "recording_limit_seconds",
                        "Recording limit (seconds)",
                    ));
                    content.push(self.field(
                        context,
                        "paste_delay_ms",
                        "Paste delay (milliseconds)",
                    ));
                }
            }
            2 => {
                let key_controls = if self.key_edit {
                    StackPanel::new().spacing(5.0).children((
                        PasswordBox::new()
                            .automation_name("API key")
                            .password(&self.password)
                            .on_password_changed(context.callback(Msg::Password)),
                        StackPanel::new()
                            .orientation(Orientation::Horizontal)
                            .spacing(6.0)
                            .children((
                                Button::new()
                                    .on_click(context.message(Msg::SaveKey))
                                    .content("Save key"),
                                Button::new()
                                    .on_click(context.message(Msg::EditKey(false)))
                                    .content("Cancel"),
                            )),
                    ))
                } else {
                    StackPanel::new().spacing(5.0).children((
                        TextBlock::new().text(&self.key_hint),
                        StackPanel::new()
                            .orientation(Orientation::Horizontal)
                            .spacing(6.0)
                            .children((
                                Button::new()
                                    .automation_name("Update API key")
                                    .on_click(context.message(Msg::EditKey(true)))
                                    .content("Update"),
                                Button::new()
                                    .automation_name("Remove API key")
                                    .on_click(context.message(Msg::RemoveKey))
                                    .content("Remove"),
                            )),
                    ))
                };
                content.push(
                    Grid::new()
                        .columns([GridLength::STAR, GridLength::STAR])
                        .column_spacing(24.0)
                        .children((
                            StackPanel::new().grid_column(0).spacing(6.0).children((
                                TextBlock::new().text("API base URL"),
                                self.model_input(context, "api_base_url", "API base URL", true),
                            )),
                            StackPanel::new()
                                .grid_column(1)
                                .spacing(6.0)
                                .children((TextBlock::new().text("API key"), key_controls)),
                        )),
                );
                content.push(self.model_group(
                    context,
                    "transcription_model",
                    "Transcription model",
                    "transcription_warmup",
                    "Warm transcription model",
                    "Warm-up sends 1 second of silence; provider charges may apply.",
                ));
                content.push(self.model_group(
                    context,
                    "llm_model",
                    "Cleanup model",
                    "cleanup_enabled",
                    "Text cleanup",
                    "When disabled, paste the original transcript without an extra model call.",
                ));
                content.push(
                    Button::new()
                        .on_click(context.message(Msg::SaveModels))
                        .content("Save model settings"),
                );
            }
            _ => {
                content.push(row(
                    caption("Languages", "Select the languages you speak"),
                    Button::new()
                        .on_click(context.message(Msg::Page(0)))
                        .content("Done"),
                ));
                content.push(
                    TextBox::new()
                        .automation_name("Search languages")
                        .placeholder_text("Search languages")
                        .text(
                            self.drafts
                                .get("language_filter")
                                .cloned()
                                .unwrap_or_default(),
                        )
                        .on_text_changed(
                            context.callback(|value| Msg::Draft("language_filter", value)),
                        )
                        .into(),
                );
                let selected: Vec<String> = serde_json::from_str(
                    self.settings
                        .get("languages")
                        .map(String::as_str)
                        .unwrap_or("[]"),
                )
                .unwrap_or_default();
                let filter = self
                    .drafts
                    .get("language_filter")
                    .cloned()
                    .unwrap_or_default()
                    .to_lowercase();
                let mut choices = Vec::new();
                for (code, label) in std::iter::once(("auto", "Auto-detect languages"))
                    .chain(models::LANGUAGES.iter().copied())
                {
                    if code != "auto" && !label.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let code = code.to_owned();
                    let checked = selected.contains(&code);
                    choices.push(
                        CheckBox::new()
                            .is_checked(checked)
                            .on_is_checked_changed(
                                context.callback(move |value| Msg::Language(code.clone(), value)),
                            )
                            .content(label),
                    );
                }
                content.push(
                    ScrollViewer::new().height(230.0).content(
                        StackPanel::new().spacing(2.0).keyed_children(
                            choices
                                .into_iter()
                                .enumerate()
                                .map(|(index, view)| (index, view)),
                        ),
                    ),
                );
            }
        }
        let mut body = vec![row(
            TextBlock::new().text("Settings").font_size(20.0).into(),
            nav,
        )];
        if !self.error.is_empty() {
            body.push(
                TextBlock::new()
                    .text(format!("Error: {}", self.error))
                    .text_wrapping(TextWrapping::Wrap)
                    .into(),
            );
        } else if !matches!(self.status.as_str(), "Ready" | "Saved" | "idle" | "") {
            body.push(
                TextBlock::new()
                    .text(&self.status)
                    .font_size(12.0)
                    .text_wrapping(TextWrapping::Wrap)
                    .into(),
            );
        }
        body.extend(content);
        ScrollViewer::new().content(
            StackPanel::new()
                .margin(Thickness::uniform(20.0))
                .spacing(12.0)
                .keyed_children(
                    body.into_iter()
                        .enumerate()
                        .map(|(index, view)| (index, view)),
                ),
        )
    }
}
fn caption(label: &str, description: &str) -> View {
    if description.is_empty() {
        return TextBlock::new()
            .text(label)
            .vertical_alignment(VerticalAlignment::Center)
            .into();
    }
    StackPanel::new()
        .spacing(3.0)
        .vertical_alignment(VerticalAlignment::Center)
        .children((
            TextBlock::new().text(label),
            TextBlock::new()
                .text(description)
                .font_size(12.0)
                .text_wrapping(TextWrapping::Wrap),
        ))
}
fn row(left: View, right: View) -> View {
    Grid::new()
        .columns([GridLength::STAR, GridLength::Auto])
        .column_spacing(18.0)
        .children((
            Border::new()
                .grid_column(0)
                .vertical_alignment(VerticalAlignment::Center)
                .content(left),
            Border::new()
                .grid_column(1)
                .vertical_alignment(VerticalAlignment::Center)
                .content(right),
        ))
}
pub fn run() -> Result<(), String> {
    let _instance = match dictto_native::instance::acquire()? {
        Some(guard) => guard,
        None => {
            return Err("Dictto is already running. Open its tray icon to view settings.".into())
        }
    };
    let smoke = std::env::args().any(|arg| arg == "--smoke-test");
    let data =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?)
            .join("com.dictto.app");
    let (tx, rx) = mpsc::channel();
    let overlay_slot = Arc::new(Mutex::new(None::<crate::overlay::NativeOverlay>));
    let sink_overlay = overlay_slot.clone();
    let runtime = NativeRuntime::new(data, move |name, payload| {
        if let Some(overlay) = sink_overlay.lock().unwrap().as_ref() {
            overlay.send(name, payload.clone());
        }
        if name != "audio-level" && name != "audio-levels" && name != "waveform-data" {
            let _ = tx.send((name.into(), payload));
        }
    })?;
    let stop = runtime.clone();
    let cancel = runtime.clone();
    let overlay = crate::overlay::NativeOverlay::spawn(
        Arc::new(move || stop.stop_recording()),
        Arc::new(move || cancel.cancel_recording()),
    )?;
    *overlay_slot.lock().unwrap() = Some(overlay.clone());
    let startup_error = if smoke {
        String::new()
    } else {
        runtime.start().err().unwrap_or_default()
    };
    if std::env::args().any(|arg| arg == "--autostart") {
        prepare_hidden_startup()?;
    }
    let result = App::run_component::<Settings>(Input {
        runtime: runtime.clone(),
        events: Arc::new(Mutex::new(rx)),
        startup_error,
        smoke,
    })
    .map_err(|e| e.to_string());
    release_startup_hook();
    runtime.shutdown();
    overlay.shutdown();
    result
}
fn icon_path() -> Option<&'static str> {
    static ICON: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let path = std::env::current_exe().ok()?.with_file_name("icon.ico");
        path.exists().then(|| path.to_string_lossy().into_owned())
    })
    .as_deref()
}
fn confirm_remove() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        MessageBoxW(
            Some(HWND(MAIN.load(Ordering::Acquire) as *mut _)),
            windows::core::w!("Remove the saved API key from Windows Credential Manager?"),
            windows::core::w!("Dictto Native"),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};
static QUITTING: AtomicBool = AtomicBool::new(false);
static MAIN: AtomicIsize = AtomicIsize::new(0);
static PREVIOUS: AtomicIsize = AtomicIsize::new(0);
static TRAY: AtomicIsize = AtomicIsize::new(0);
static TRAY_ICON: AtomicIsize = AtomicIsize::new(0);
static TRAY_ICON_OWNED: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static START_HIDDEN: AtomicBool = AtomicBool::new(false);
static STARTUP_HOOK: AtomicIsize = AtomicIsize::new(0);
const TRAY_MESSAGE: u32 = WM_APP + 42;
const HIDE_STARTUP: u32 = WM_APP + 43;
unsafe extern "system" fn startup_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Reactor 0.100 always activates its first window before publishing effects.
    if code == HCBT_ACTIVATE as i32 && START_HIDDEN.load(Ordering::Acquire) {
        return LRESULT(1);
    }
    CallNextHookEx(None, code, wparam, lparam)
}
fn prepare_hidden_startup() -> Result<(), String> {
    unsafe {
        START_HIDDEN.store(true, Ordering::Release);
        let hook = SetWindowsHookExW(
            WH_CBT,
            Some(startup_hook),
            None,
            windows::Win32::System::Threading::GetCurrentThreadId(),
        )
        .map_err(|e| e.to_string())?;
        STARTUP_HOOK.store(hook.0 as isize, Ordering::Release);
        Ok(())
    }
}
fn release_startup_hook() {
    START_HIDDEN.store(false, Ordering::Release);
    let hook = STARTUP_HOOK.swap(0, Ordering::AcqRel);
    if hook != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(hook as *mut _));
        }
    }
}
unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == HIDE_STARTUP {
        let _ = ShowWindow(hwnd, SW_HIDE);
        release_startup_hook();
        return LRESULT(0);
    }
    if message == WM_CLOSE && !QUITTING.load(Ordering::Acquire) {
        let _ = ShowWindow(hwnd, SW_HIDE);
        return LRESULT(0);
    }
    let previous: WNDPROC = Some(std::mem::transmute(PREVIOUS.load(Ordering::Acquire)));
    CallWindowProcW(previous, hwnd, message, wparam, lparam)
}
unsafe extern "system" fn tray_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message != 0 && message == TASKBAR_CREATED.load(Ordering::Acquire) {
        if !Shell_NotifyIconW(NIM_ADD, &tray_data(hwnd)).as_bool() {
            // Keep settings reachable if Explorer rejects icon restoration.
            let main = HWND(MAIN.load(Ordering::Acquire) as *mut _);
            let _ = ShowWindow(main, SW_SHOWNOACTIVATE);
        }
        return LRESULT(0);
    }
    if message == TRAY_MESSAGE {
        if lparam.0 as u32 == WM_LBUTTONUP || lparam.0 as u32 == WM_LBUTTONDBLCLK {
            let main = HWND(MAIN.load(Ordering::Acquire) as *mut _);
            let _ = ShowWindow(main, SW_SHOW);
            let _ = SetForegroundWindow(main);
        }
        if lparam.0 as u32 == WM_RBUTTONUP {
            if let Ok(menu) = CreatePopupMenu() {
                let _ = AppendMenuW(menu, MF_STRING, 1, w!("Settings"));
                let _ = AppendMenuW(menu, MF_STRING, 2, w!("Quit Dictto"));
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let _ = SetForegroundWindow(hwnd);
                let chosen =
                    TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, Some(0), hwnd, None);
                let _ = DestroyMenu(menu);
                let main = HWND(MAIN.load(Ordering::Acquire) as *mut _);
                if chosen.0 == 1 {
                    let _ = ShowWindow(main, SW_SHOW);
                    let _ = SetForegroundWindow(main);
                } else if chosen.0 == 2 {
                    QUITTING.store(true, Ordering::Release);
                    let _ = PostMessageW(Some(main), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}
fn install_tray() -> Result<(), String> {
    unsafe {
        let main = FindWindowW(None, w!("Dictto Native")).map_err(|e| e.to_string())?;
        MAIN.store(main.0 as isize, Ordering::Release);
        let class = w!("DicttoNativeTray");
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(tray_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("Dictto Native Tray"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        TRAY.store(hwnd.0 as isize, Ordering::Release);
        let taskbar_message = RegisterWindowMessageW(w!("TaskbarCreated"));
        if taskbar_message == 0 {
            return Err("Could not register Explorer tray recovery".into());
        }
        TASKBAR_CREATED.store(taskbar_message, Ordering::Release);
        let icon = if let Some(path) = icon_path() {
            let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            LoadImageW(
                None,
                PCWSTR(wide.as_ptr()),
                IMAGE_ICON,
                32,
                32,
                LR_LOADFROMFILE,
            )
            .ok()
            .map(|h| {
                TRAY_ICON_OWNED.store(true, Ordering::Release);
                HICON(h.0)
            })
            .unwrap_or_else(|| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
        } else {
            LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
        };
        TRAY_ICON.store(icon.0 as isize, Ordering::Release);
        if !Shell_NotifyIconW(NIM_ADD, &tray_data(hwnd)).as_bool() {
            return Err("Windows could not add the Dictto tray icon".into());
        }
        // Hide on close only after the tray provides a way to reopen settings.
        let previous = SetWindowLongPtrW(main, GWLP_WNDPROC, settings_proc as *const () as isize);
        if previous == 0 {
            return Err("Could not install settings window close handler".into());
        }
        PREVIOUS.store(previous, Ordering::Release);
        if START_HIDDEN.load(Ordering::Acquire) {
            // The posted hide runs after initial WinUI activation and keeps the tray alive.
            let _ = ShowWindow(main, SW_HIDE);
            PostMessageW(Some(main), HIDE_STARTUP, WPARAM(0), LPARAM(0))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_MESSAGE,
        hIcon: HICON(TRAY_ICON.load(Ordering::Acquire) as *mut _),
        ..Default::default()
    };
    let tip: Vec<u16> = "Dictto Native".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}
fn remove_tray() {
    unsafe {
        let hwnd = HWND(TRAY.swap(0, Ordering::AcqRel) as *mut _);
        if hwnd.0.is_null() {
            return;
        }
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        let _ = DestroyWindow(hwnd);
        let icon = TRAY_ICON.swap(0, Ordering::AcqRel);
        if icon != 0 && TRAY_ICON_OWNED.swap(false, Ordering::AcqRel) {
            let _ = DestroyIcon(HICON(icon as *mut _));
        }
    }
}
