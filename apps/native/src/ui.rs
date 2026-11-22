use dictto_core::events;
use dictto_core::Runtime;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicIsize, Ordering},
        mpsc, Arc, Mutex,
    },
};
use windows::{
    core::{w, BOOL, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
};
use dictto_core::settings_model as models;
#[path = "ui_speech.rs"]
mod speech;
#[path = "ui_api.rs"]
mod api;
#[path = "ui_keys.rs"]
mod keys;
use dictto_native::speech_api::SpeechModel;

const TRAY: u32 = WM_APP + 42;
const EVENTS: u32 = WM_APP + 43;
const OPEN: u32 = WM_APP + 44;
const STATUS: usize = 90;
// Secondary text, drawn dimmer than labels.
const HINT: usize = 91;
const TAB: usize = 10;
const TABS: &[&str] = &["General", "Dictation", "Voice", "Shortcuts", "Connection"];
const GENERAL: usize = 0;
const DICTATION: usize = 1;
const VOICE: usize = 2;
const SHORTCUTS: usize = 3;
const CONNECTION: usize = 4;
// Reached from General, not from a tab.
const LANGUAGES: usize = 5;
const CHOOSE_LANGUAGES: usize = 16;
const LANGUAGES_DONE: usize = 17;
const CHECKS: &[(usize, &str)] = &[
    (100, "auto_start"),
    (101, "sound_effects_enabled"),
    (102, "transcription_warmup"),
    (103, "cleanup_enabled"),
];
const MICROPHONE: usize = 400;
const RECORDING_LIMIT: usize = 204;
const PASTE_DELAY: usize = 205;
const LIMITS: &[(&str, &str)] = &[
    ("60", "1 minute"),
    ("120", "2 minutes"),
    ("300", "5 minutes"),
    ("600", "10 minutes"),
    ("1800", "30 minutes"),
];
const DELAYS: &[(&str, &str)] = &[
    ("0", "None"),
    ("50", "50 ms"),
    ("150", "150 ms"),
    ("300", "300 ms"),
    ("500", "500 ms"),
];
// Client area in 96-dpi pixels; content spans x 20..634, controls start at x 240.
const WIDTH: i32 = 654;
const HEIGHT: i32 = 470;

struct App {
    runtime: Runtime,
    events: mpsc::Receiver<(String, Value)>,
    host: HWND,
    window: HWND,
    children: Vec<(usize, HWND)>,
    // Combo id -> (setting key, value of each item): picking an item saves it.
    bound: HashMap<usize, (String, Vec<String>)>,
    drafts: HashMap<String, String>,
    page: usize,
    font: HFONT,
    background: HBRUSH,
    input_background: HBRUSH,
    dark: bool,
    dpi: u32,
    icon: HICON,
    taskbar_message: u32,
    speech_models: Vec<SpeechModel>,
    catalog_generation: u64,
    catalog_loading: bool,
    models: api::Models,
    engine_fields: Vec<(String, String)>,
    capture: Option<String>,
    languages: Vec<String>,
    smoke: bool,
    status: String,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn dark_theme() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|value| value == 0)
        .unwrap_or(false)
}

impl App {
    fn new(runtime: Runtime, events: mpsc::Receiver<(String, Value)>, icon: HICON, smoke: bool) -> Self {
        App {
            runtime,
            events,
            host: HWND::default(),
            window: HWND::default(),
            children: Vec::new(),
            bound: HashMap::new(),
            drafts: HashMap::new(),
            page: GENERAL,
            font: HFONT::default(),
            background: HBRUSH::default(),
            input_background: HBRUSH::default(),
            dark: false,
            dpi: 96,
            icon,
            taskbar_message: 0,
            speech_models: Vec::new(),
            catalog_generation: 0,
            catalog_loading: false,
            models: Default::default(),
            engine_fields: Vec::new(),
            capture: None,
            languages: Vec::new(),
            smoke,
            status: String::new(),
        }
    }
    fn scale(&self, value: i32) -> i32 {
        value * self.dpi as i32 / 96
    }
    fn control(&self, id: usize) -> HWND {
        self.children
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, hwnd)| *hwnd)
            .unwrap_or_default()
    }
    unsafe fn text(&self, id: usize) -> String {
        let hwnd = self.control(id);
        let mut buffer = vec![0; GetWindowTextLengthW(hwnd) as usize + 1];
        let count = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..count as usize])
    }
    unsafe fn label_text(&self, id: usize, text: &str) {
        let _ = SetWindowTextW(self.control(id), PCWSTR(wide(text).as_ptr()));
    }
    fn setting(&self, key: &str) -> &str {
        self.drafts.get(key).map_or("", String::as_str)
    }
    unsafe fn add(
        &mut self,
        id: usize,
        class: PCWSTR,
        text: &str,
        style: WINDOW_STYLE,
        rect: [i32; 4],
    ) -> HWND {
        let [x, y, width, height] = rect.map(|v| self.scale(v));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            width,
            height,
            Some(self.window),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )
        .expect("create settings control");
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(self.font.0 as usize)),
            Some(LPARAM(1)),
        );
        self.children.push((id, hwnd));
        hwnd
    }
    unsafe fn label(&mut self, text: &str, rect: [i32; 4]) {
        self.add(0, w!("STATIC"), text, WINDOW_STYLE::default(), rect);
    }
    unsafe fn hint(&mut self, text: &str, rect: [i32; 4]) {
        self.add(HINT, w!("STATIC"), text, WINDOW_STYLE::default(), rect);
    }
    unsafe fn button(&mut self, id: usize, text: &str, rect: [i32; 4]) {
        self.add(
            id,
            w!("BUTTON"),
            text,
            WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            rect,
        );
    }
    unsafe fn check(&mut self, id: usize, text: &str, value: bool, rect: [i32; 4]) {
        let hwnd = self.add(
            id,
            w!("BUTTON"),
            text,
            WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            rect,
        );
        // Classic checkboxes accept our system-theme colours without a XAML renderer.
        let _ = SetWindowTheme(hwnd, w!(""), w!(""));
        SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM(value as usize)), None);
    }
    unsafe fn setting_check(&mut self, id: usize, text: &str, rect: [i32; 4]) {
        let key = CHECKS.iter().find(|(i, _)| *i == id).unwrap().1;
        let value = self.setting(key) == "true";
        self.check(id, text, value, rect);
    }
    unsafe fn edit(&mut self, id: usize, value: &str, cue: &str, rect: [i32; 4]) -> HWND {
        let hwnd = self.add(
            id,
            w!("EDIT"),
            value,
            WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            rect,
        );
        self.cue(hwnd, cue);
        hwnd
    }
    unsafe fn cue(&self, hwnd: HWND, cue: &str) {
        SendMessageW(
            hwnd,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(wide(cue).as_ptr() as isize)),
        );
    }
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
    /// A list that saves `key` when an item is picked. A saved value missing from
    /// `options` stays first, labelled with `missing`, so opening never changes it.
    unsafe fn bound_combo(
        &mut self,
        id: usize,
        key: &str,
        mut options: Vec<(String, String)>,
        missing: &str,
        rect: [i32; 4],
    ) {
        let saved = self.setting(key).to_string();
        if !options.iter().any(|(value, _)| *value == saved) {
            let label = if missing.is_empty() { saved.clone() } else { format!("{saved} {missing}") };
            options.insert(0, (saved.clone(), label));
        }
        let selected = options.iter().position(|(value, _)| *value == saved).unwrap_or(0);
        let labels: Vec<String> = options.iter().map(|(_, label)| label.clone()).collect();
        self.combo(id, &labels, selected, rect);
        self.bound
            .insert(id, (key.into(), options.into_iter().map(|(value, _)| value).collect()));
    }
    unsafe fn theme(&mut self) {
        self.dark = dark_theme();
        let _ = DeleteObject(self.background.into());
        let _ = DeleteObject(self.input_background.into());
        self.background = CreateSolidBrush(COLORREF(if self.dark { 0x202020 } else { 0xf3f3f3 }));
        self.input_background =
            CreateSolidBrush(COLORREF(if self.dark { 0x303030 } else { 0xffffff }));
        if !self.window.0.is_null() {
            let dark = BOOL(self.dark as i32);
            let _ = DwmSetWindowAttribute(
                self.window,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as _,
                std::mem::size_of::<BOOL>() as u32,
            );
            let _ = InvalidateRect(Some(self.window), None, true);
        }
    }
    /// Asks the endpoint for its models once per endpoint and key; the lists then stay
    /// until Refresh, across window closes.
    fn autoload(&mut self) {
        if self.smoke || !self.runtime.has_key() {
            return;
        }
        self.autoload_models();
        if self.speech_models.is_empty() && !self.catalog_loading {
            self.refresh_speech_models();
        }
    }
    unsafe fn show(&mut self) -> Result<(), String> {
        if !self.window.0.is_null() {
            let _ = ShowWindow(self.window, SW_RESTORE);
            let _ = SetForegroundWindow(self.window);
            return Ok(());
        }
        self.drafts = self.runtime.get_settings()?;
        self.page = if self.runtime.has_key() { GENERAL } else { CONNECTION };
        self.autoload();
        self.dpi = GetDpiForSystem();
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: self.scale(WIDTH),
            bottom: self.scale(HEIGHT),
        };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        AdjustWindowRectExForDpi(
            &mut rect,
            style,
            false,
            WINDOW_EX_STYLE::default(),
            self.dpi,
        )
        .map_err(|e| e.to_string())?;
        self.window = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("Dictto.Win32.Settings"),
            w!("Dictto Settings"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            None,
            Some(self as *mut Self as _),
        )
        .map_err(|e| e.to_string())?;
        self.recreate_font();
        self.theme();
        self.render();
        SendMessageW(
            self.window,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(self.icon.0 as isize)),
        );
        let _ = ShowWindow(self.window, SW_SHOW);
        let _ = SetForegroundWindow(self.window);
        Ok(())
    }
    unsafe fn recreate_font(&mut self) {
        let _ = DeleteObject(self.font.into());
        self.font = CreateFontW(
            -self.scale(14),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        );
    }
    unsafe fn render(&mut self) {
        self.runtime.set_hotkey_capture(false);
        self.capture = None;
        // Taken first: a focused edit reports EN_KILLFOCUS while it is destroyed, and its
        // handler must then find no control rather than read a half-destroyed one.
        for (_, hwnd) in std::mem::take(&mut self.children) {
            let _ = DestroyWindow(hwnd);
        }
        self.bound.clear();
        let tab_page = if self.page == LANGUAGES { GENERAL } else { self.page };
        let has_key = self.runtime.has_key();
        for (i, name) in TABS.iter().enumerate() {
            let name = if i == CONNECTION && !has_key { "Connection (!)" } else { name };
            let group = if i == 0 { WS_GROUP } else { WINDOW_STYLE::default() };
            let hwnd = self.add(
                TAB + i,
                w!("BUTTON"),
                name,
                WS_TABSTOP | group | WINDOW_STYLE((BS_AUTORADIOBUTTON | BS_PUSHLIKE) as u32),
                [20 + i as i32 * 123, 14, 117, 30],
            );
            SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM((i == tab_page) as usize)), None);
        }
        match self.page {
            GENERAL => self.render_general(),
            DICTATION => self.render_dictation(),
            VOICE => self.render_speech(),
            SHORTCUTS => self.render_shortcuts(),
            CONNECTION => self.render_connection(),
            _ => self.render_languages(),
        }
        self.add(
            STATUS,
            w!("STATIC"),
            &self.status.clone(),
            WINDOW_STYLE::default(),
            [20, HEIGHT - 40, 614, 34],
        );
        let _ = InvalidateRect(Some(self.window), None, true);
    }
    unsafe fn render_general(&mut self) {
        self.setting_check(100, "Start Dictto when you sign in to Windows", [20, 66, 614, 24]);
        self.setting_check(101, "Play a sound when recording starts and stops", [20, 98, 614, 24]);
        self.label("Spoken languages", [20, 148, 210, 22]);
        let languages = self.selected_languages();
        let summary = if languages.is_empty() || languages.iter().any(|v| v == "auto") {
            "Auto-detect".into()
        } else {
            languages
                .iter()
                .map(|code| {
                    models::LANGUAGES
                        .iter()
                        .find(|(c, _)| c == code)
                        .map_or(code.as_str(), |(_, name)| name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.label(&summary, [240, 148, 296, 22]);
        self.button(CHOOSE_LANGUAGES, "Choose...", [546, 143, 88, 30]);
        self.hint(
            "Naming your languages helps the model; auto-detect guesses for each dictation.",
            [240, 176, 394, 36],
        );
        self.label("Microphone", [20, 232, 210, 22]);
        let current = self.runtime.current_microphone();
        let mut options = vec![(
            "auto-detect".to_string(),
            if current.is_empty() { "Auto-detect".into() } else { format!("Auto-detect ({current})") },
        )];
        match self.runtime.microphones() {
            Ok(items) => options.extend(items.into_iter().map(|m| (m.name.clone(), m.name))),
            Err(e) => self.status = e,
        }
        self.bound_combo(MICROPHONE, "microphone_device", options, "(not connected)", [240, 228, 394, 250]);
        self.hint("Auto-detect follows the Windows default input device.", [240, 260, 394, 22]);
    }
    unsafe fn render_dictation(&mut self) {
        self.render_transcription_model(66);
        self.setting_check(102, "Warm up the model so the first words come back faster", [240, 102, 394, 24]);
        self.hint(
            "Sends 1 second of silence at startup and when recording starts; provider charges may apply.",
            [240, 128, 394, 36],
        );
        self.label("Text cleanup", [20, 180, 210, 22]);
        self.setting_check(103, "Fix punctuation and remove filler words", [240, 177, 394, 24]);
        self.render_cleanup_model(210);
        let _ = EnableWindow(self.control(api::CLEANUP_MODEL), self.setting("cleanup_enabled") == "true");
        self.hint("When cleanup is off, the raw transcript is pasted.", [240, 242, 394, 22]);
        self.label("Maximum recording", [20, 290, 210, 22]);
        let limits = LIMITS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
        self.bound_combo(RECORDING_LIMIT, "recording_limit_seconds", limits, "seconds", [240, 286, 180, 200]);
        self.label("Paste delay", [20, 330, 210, 22]);
        let delays = DELAYS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
        self.bound_combo(PASTE_DELAY, "paste_delay_ms", delays, "ms", [240, 326, 180, 200]);
        self.hint("Raise it if the start of pasted text goes missing.", [240, 358, 394, 22]);
    }
    fn selected_languages(&self) -> Vec<String> {
        serde_json::from_str(self.drafts.get("languages").map_or("[]", String::as_str))
            .unwrap_or_default()
    }
    unsafe fn render_languages(&mut self) {
        self.label("Spoken languages (select all that apply)", [20, 66, 410, 22]);
        self.button(LANGUAGES_DONE, "Done", [546, 62, 88, 30]);
        let values = self.selected_languages();
        self.check(
            501,
            "Auto-detect",
            values.is_empty() || values.iter().any(|v| v == "auto"),
            [20, 100, 180, 28],
        );
        self.label("Search", [220, 103, 65, 22]);
        self.edit(502, "", "Language name", [290, 100, 344, 28]);
        self.add(
            500,
            w!("LISTBOX"),
            "",
            WS_TABSTOP
                | WS_BORDER
                | WS_VSCROLL
                | WINDOW_STYLE((LBS_MULTIPLESEL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
            [20, 140, 614, 280],
        );
        self.fill_languages("");
    }
    unsafe fn fill_languages(&mut self, filter: &str) {
        let list = self.control(500);
        SendMessageW(list, LB_RESETCONTENT, None, None);
        self.languages.clear();
        let selected = self.selected_languages();
        for &(code, name) in models::LANGUAGES {
            if !name.to_lowercase().contains(&filter.to_lowercase())
                && !code.contains(&filter.to_lowercase())
            {
                continue;
            }
            let index = self.languages.len();
            self.languages.push(code.into());
            SendMessageW(
                list,
                LB_ADDSTRING,
                None,
                Some(LPARAM(wide(name).as_ptr() as isize)),
            );
            if selected.iter().any(|s| s == code) {
                SendMessageW(
                    list,
                    LB_SETSEL,
                    Some(WPARAM(1)),
                    Some(LPARAM(index as isize)),
                );
            }
        }
    }
    unsafe fn persist(&mut self, key: &str, value: &str) -> bool {
        match self.runtime.set_setting(key, value) {
            Ok(()) => {
                self.drafts.insert(key.into(), value.into());
                if key == "api_base_url" {
                    self.forget_catalogs();
                    self.autoload();
                }
                self.status = "Saved".into();
                true
            }
            Err(e) => {
                self.status = e;
                false
            }
        }
    }
    /// Lists that belong to the previous endpoint or key.
    fn forget_catalogs(&mut self) {
        self.models.forget();
        self.speech_models.clear();
        self.catalog_generation += 1;
        self.catalog_loading = false;
    }
    unsafe fn command(&mut self, id: usize, notification: u32) {
        if self.api_command(id, notification)
            || self.speech_command(id, notification)
            || self.shortcut_command(id, notification)
        {
            if !self.window.0.is_null() {
                self.label_text(STATUS, &self.status);
            }
            return;
        }
        if id == 502 && notification == EN_CHANGE {
            self.fill_languages(&self.text(502));
            return;
        }
        // LBN_SELCHANGE has the same value, so the list box must not be caught here.
        if notification == CBN_SELCHANGE && self.bound.contains_key(&id) {
            if let Some((key, values)) = self.bound.get(&id).cloned() {
                let index = SendMessageW(self.control(id), CB_GETCURSEL, None, None).0 as usize;
                if let Some(value) = values.get(index) {
                    if !self.persist(&key, value) {
                        self.render();
                    }
                }
            }
        } else if id == 500 && notification == LBN_SELCHANGE {
            let mut values = self.selected_languages();
            values.retain(|v| v != "auto" && !self.languages.contains(v));
            for (index, code) in self.languages.iter().enumerate() {
                if SendMessageW(self.control(500), LB_GETSEL, Some(WPARAM(index)), None).0 > 0 {
                    values.push(code.clone());
                }
            }
            self.persist("languages", &serde_json::to_string(&values).unwrap());
            SendMessageW(
                self.control(501),
                BM_SETCHECK,
                Some(WPARAM(values.is_empty() as usize)),
                None,
            );
        } else if notification == BN_CLICKED {
            self.capture_engine_draft();
            match id {
                _ if (TAB..TAB + TABS.len()).contains(&id)
                    || id == CHOOSE_LANGUAGES
                    || id == LANGUAGES_DONE =>
                {
                    self.page = match id {
                        CHOOSE_LANGUAGES => LANGUAGES,
                        LANGUAGES_DONE => GENERAL,
                        _ => id - TAB,
                    };
                    self.render();
                    let focus = match id {
                        LANGUAGES_DONE => CHOOSE_LANGUAGES,
                        CHOOSE_LANGUAGES => 502,
                        _ => id,
                    };
                    let _ = SetFocus(Some(self.control(focus)));
                }
                100..=103 => {
                    let key = CHECKS.iter().find(|(i, _)| *i == id).unwrap().1;
                    let checked = SendMessageW(self.control(id), BM_GETCHECK, None, None).0 != 0;
                    if !self.persist(key, if checked { "true" } else { "false" }) {
                        SendMessageW(
                            self.control(id),
                            BM_SETCHECK,
                            Some(WPARAM(!checked as usize)),
                            None,
                        );
                    }
                    if id == 103 {
                        let _ = EnableWindow(
                            self.control(api::CLEANUP_MODEL),
                            self.setting(key) == "true",
                        );
                    }
                }
                501 => {
                    let on = SendMessageW(self.control(501), BM_GETCHECK, None, None).0 != 0;
                    if on {
                        self.persist("languages", "[]");
                        self.fill_languages(&self.text(502));
                    } else {
                        self.status = "Select one or more languages below.".into();
                    }
                }
                _ => {}
            }
        }
        if !self.window.0.is_null() {
            self.label_text(STATUS, &self.status);
        }
    }
    unsafe fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.host,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip = wide("Dictto Native");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        data
    }
    unsafe fn drain_events(&mut self) {
        let mut devices_changed = false;
        while let Ok((name, payload)) = self.events.try_recv() {
            if name == events::SPEECH_CATALOG {
                self.receive_speech_models(payload);
                continue;
            }
            if name == events::MODEL_CATALOG {
                self.receive_models(payload);
                continue;
            }
            if name == events::AUDIO_DEVICES_CHANGED {
                devices_changed = true;
            }
            if name == events::SPEECH_ERROR {
                self.status = payload["message"].as_str().unwrap_or("Speech action failed.").into();
            } else if matches!(
                name.as_str(),
                "error" | "recording-error" | events::DICTATION_ERROR
            ) {
                self.status = dictto_core::transcription::friendly_error(
                    payload["message"].as_str().unwrap_or_default()).into();
            } else if matches!(
                name.as_str(),
                "recording-state" | events::RECORDING_STATE_CHANGED | "processing-state"
            ) {
                self.status = payload["state"]
                    .as_str()
                    .or_else(|| payload.as_str())
                    .unwrap_or("")
                    .into();
            }
        }
        if devices_changed && self.page == GENERAL && !self.window.0.is_null() {
            self.render();
        }
        if !self.window.0.is_null() {
            self.label_text(STATUS, &self.status);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    match message {
        WM_COMMAND => {
            (*pointer).command(wparam.0 & 0xffff, ((wparam.0 >> 16) & 0xffff) as u32);
            LRESULT(0)
        }
        WM_CLOSE => {
            (*pointer).runtime.set_hotkey_capture(false);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            (*pointer).window = HWND::default();
            (*pointer).children.clear();
            (*pointer).bound.clear();
            let _ = DeleteObject((*pointer).font.into());
            (*pointer).font = HFONT::default();
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_ERASEBKGND => {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            FillRect(HDC(wparam.0 as _), &rect, (*pointer).background);
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            let dc = HDC(wparam.0 as _);
            let dark = (*pointer).dark;
            let id = GetDlgCtrlID(HWND(lparam.0 as _)) as usize;
            // Read-only shortcut fields report as static but should look like inputs.
            let input = message == WM_CTLCOLOREDIT
                || message == WM_CTLCOLORLISTBOX
                || keys::is_field(id);
            let text = match (id == HINT, dark) {
                (true, true) => 0xa8a8a8,
                (true, false) => 0x606060,
                (false, true) => 0xf0f0f0,
                (false, false) => 0x202020,
            };
            SetTextColor(dc, COLORREF(text));
            SetBkColor(
                dc,
                COLORREF(if dark {
                    if input {
                        0x303030
                    } else {
                        0x202020
                    }
                } else {
                    if input {
                        0xffffff
                    } else {
                        0xf3f3f3
                    }
                }),
            );
            LRESULT(if input {
                (*pointer).input_background.0 as isize
            } else {
                (*pointer).background.0 as isize
            })
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            (*pointer).theme();
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_DPICHANGED => {
            (*pointer).capture_engine_draft();
            (*pointer).dpi = (wparam.0 & 0xffff) as u32;
            let rect = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            // Destroy children before replacing their font handle.
            for (_, child) in (*pointer).children.drain(..) {
                let _ = DestroyWindow(child);
            }
            (*pointer).recreate_font();
            (*pointer).render();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lparam.0 as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    if message != 0 && message == (*pointer).taskbar_message {
        if !Shell_NotifyIconW(NIM_ADD, &(*pointer).tray_data()).as_bool() {
            let _ = (*pointer).show();
        }
        return LRESULT(0);
    }
    match message {
        WM_WTSSESSION_CHANGE => {
            match wparam.0 as u32 {
                WTS_SESSION_LOCK | WTS_SESSION_LOGOFF | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => (*pointer).runtime.session_changed(true),
                WTS_SESSION_UNLOCK | WTS_SESSION_LOGON | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => (*pointer).runtime.session_changed(false),
                _=>{}
            }
            LRESULT(0)
        }
        WM_ENDSESSION if wparam.0!=0 => {
            (*pointer).runtime.session_changed(true);
            LRESULT(0)
        }
        EVENTS => {
            (*pointer).drain_events();
            LRESULT(0)
        }
        OPEN => {
            if let Err(e) = (*pointer).show() {
                (*pointer).status = e;
            }
            LRESULT(0)
        }
        TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    let _ = PostMessageW(Some(hwnd), OPEN, WPARAM(0), LPARAM(0));
                }
                WM_RBUTTONUP => {
                    if let Ok(menu) = CreatePopupMenu() {
                        let _ = AppendMenuW(menu, MF_STRING, 1, w!("Settings"));
                        let _ = AppendMenuW(menu, MF_STRING, 2, w!("Quit Dictto"));
                        let mut point = POINT::default();
                        let _ = GetCursorPos(&mut point);
                        let _ = SetForegroundWindow(hwnd);
                        let chosen = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD,
                            point.x,
                            point.y,
                            Some(0),
                            hwnd,
                            None,
                        );
                        let _ = DestroyMenu(menu);
                        if chosen.0 == 1 {
                            let _ = PostMessageW(Some(hwnd), OPEN, WPARAM(0), LPARAM(0));
                        }
                        if chosen.0 == 2 {
                            PostQuitMessage(0);
                        }
                        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub fn run() -> Result<(), String> {
    let _instance = dictto_native::instance::acquire()?
        .ok_or("Dictto is already running. Open its tray icon to view settings.")?;
    let smoke = std::env::args().any(|arg| arg == "--smoke-test");
    let data =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?)
            .join("com.dictto.app");
    let (tx, rx) = mpsc::channel();
    let host_address = Arc::new(AtomicIsize::new(0));
    let sink_address = host_address.clone();
    let overlay_slot = Arc::new(Mutex::new(None::<crate::overlay::NativeOverlay>));
    let sink_overlay = overlay_slot.clone();
    let speech = dictto_windows::speech::platform(&data);
    let runtime = Runtime::with_speech(data, dictto_windows::platform(), speech, move |name, payload| {
        if let Some(overlay) = sink_overlay.lock().unwrap().as_ref() {
            overlay.send(name, payload.clone());
        }
        if matches!(
            name,
            "error"
                | events::SPEECH_ERROR
                | "recording-error"
                | events::DICTATION_ERROR
                | "recording-state"
                | events::RECORDING_STATE_CHANGED
                | "processing-state"
                | events::SPEECH_CATALOG
                | events::MODEL_CATALOG
                | events::AUDIO_DEVICES_CHANGED
        ) {
            let _ = tx.send((name.into(), payload));
            let address = sink_address.load(Ordering::Acquire);
            if address != 0 {
                unsafe {
                    let _ = PostMessageW(Some(HWND(address as _)), EVENTS, WPARAM(0), LPARAM(0));
                }
            }
        }
    })?;
    let stop = runtime.clone();
    let cancel = runtime.clone();
    let overlay = crate::overlay::NativeOverlay::spawn(
        Arc::new(move || stop.stop_recording()),
        Arc::new(move || cancel.cancel_recording()),
    )?;
    *overlay_slot.lock().unwrap() = Some(overlay.clone());
    let result = unsafe { run_windows(runtime.clone(), rx, host_address.clone(), smoke) };
    host_address.store(0, Ordering::Release);
    runtime.shutdown();
    overlay.shutdown();
    result
}

unsafe fn run_windows(
    runtime: Runtime,
    events: mpsc::Receiver<(String, Value)>,
    address: Arc<AtomicIsize>,
    smoke: bool,
) -> Result<(), String> {
    let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
    InitCommonControlsEx(&INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES,
    })
    .ok()
    .map_err(|e| e.to_string())?;
    let icon = LoadIconW(Some(instance), PCWSTR(1usize as *const u16))
        .or_else(|_| LoadIconW(None, IDI_APPLICATION))
        .map_err(|e| e.to_string())?;
    let cursor = LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?;
    for (name, proc) in [
        (
            w!("Dictto.Win32.Settings"),
            Some(window_proc as unsafe extern "system" fn(_, _, _, _) -> _),
        ),
        (
            w!("Dictto.Win32.Host"),
            Some(host_proc as unsafe extern "system" fn(_, _, _, _) -> _),
        ),
    ] {
        if RegisterClassW(&WNDCLASSW {
            lpfnWndProc: proc,
            hInstance: instance,
            hCursor: cursor,
            hIcon: icon,
            lpszClassName: name,
            ..Default::default()
        }) == 0
        {
            return Err(windows::core::Error::from_thread().to_string());
        }
    }
    let mut app = Box::new(App::new(runtime, events, icon, smoke));
    app.taskbar_message = RegisterWindowMessageW(w!("TaskbarCreated"));
    if smoke {
        app.status = "Preview: recording services disabled".into();
    }
    app.theme();
    app.host = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        w!("Dictto.Win32.Host"),
        w!("Dictto Native Tray"),
        WINDOW_STYLE::default(),
        0,
        0,
        0,
        0,
        None,
        None,
        Some(instance),
        Some((&mut *app as *mut App).cast()),
    )
    .map_err(|e| e.to_string())?;
    address.store(app.host.0 as isize, Ordering::Release);
    use windows::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    let notifications = !smoke && WTSRegisterSessionNotification(app.host, NOTIFY_FOR_THIS_SESSION).is_ok();
    if !smoke && !notifications {
        app.runtime.session_changed(true);
    }
    let result = (|| -> Result<(), String> {
        if !Shell_NotifyIconW(NIM_ADD, &app.tray_data()).as_bool() {
            return Err("Windows could not add the Dictto tray icon".into());
        }
        if !smoke {
            if let Err(error) = app.runtime.start() {
                app.status = error;
            }
        }
        if !std::env::args().any(|arg| arg == "--autostart") || !app.status.is_empty() && !smoke {
            app.show()?;
        }
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0).0;
            if result == -1 {
                return Err(windows::core::Error::from_thread().to_string());
            }
            if result == 0 {
                break;
            }
            if !app.window.0.is_null() && app.shortcut_key(&message) {
                continue;
            }
            if app.window.0.is_null() || !IsDialogMessageW(app.window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    })();
    if notifications {
        let _ = WTSUnRegisterSessionNotification(app.host);
    }
    address.store(0, Ordering::Release);
    let _ = Shell_NotifyIconW(NIM_DELETE, &app.tray_data());
    if !app.window.0.is_null() {
        let _ = DestroyWindow(app.window);
    }
    SetWindowLongPtrW(app.host, GWLP_USERDATA, 0);
    let _ = DestroyWindow(app.host);
    let _ = DeleteObject(app.background.into());
    let _ = DeleteObject(app.input_background.into());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_controls_persist_and_release_the_window_on_close() {
        let path = std::env::temp_dir().join(format!("dictto-win32-ui-{}", std::process::id()));
        {
            let runtime = Runtime::with_speech(path.clone(), dictto_windows::platform(), dictto_windows::speech::platform(&path), |_, _| {}).unwrap();
            runtime.set_setting("languages", r#"["fr","en"]"#).unwrap();
            let (events_tx, events) = mpsc::channel();
            unsafe {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: HINSTANCE(GetModuleHandleW(None).unwrap().0),
                    lpszClassName: w!("Dictto.Win32.Settings"),
                    ..Default::default()
                });
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(host_proc),
                    hInstance: HINSTANCE(GetModuleHandleW(None).unwrap().0),
                    lpszClassName: w!("Dictto.Win32.Host"),
                    ..Default::default()
                });
                let mut app = Box::new(App::new(runtime, events, HICON::default(), true));
                app.host = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    w!("Dictto.Win32.Host"),
                    w!("Test tray host"),
                    WINDOW_STYLE::default(),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    Some((&mut *app as *mut App).cast()),
                )
                .unwrap();
                app.show().unwrap();
                assert!(IsWindow(Some(app.window)).as_bool());
                let pick = |app: &mut App, id: usize, index: usize| {
                    SendMessageW(app.control(id), CB_SETCURSEL, Some(WPARAM(index)), None);
                    app.command(id, CBN_SELCHANGE);
                };
                let setting = |app: &App, key: &str| app.runtime.get_settings().unwrap()[key].clone();

                // Connection: a typed endpoint saves when focus leaves.
                app.command(TAB + CONNECTION, BN_CLICKED);
                assert_eq!(app.text(api::KEY), "");
                app.label_text(api::ENDPOINT, "http://localhost:1234/v1");
                app.command(api::ENDPOINT, CBN_KILLFOCUS);
                assert_eq!(setting(&app, "api_base_url"), "http://localhost:1234/v1");

                // Dictation: until models arrive the saved one is the only choice.
                app.command(TAB + DICTATION, BN_CLICKED);
                assert_eq!(SendMessageW(app.control(api::TRANSCRIPTION_MODEL), CB_GETCOUNT, None, None).0, 1);
                assert_eq!(app.text(api::TRANSCRIPTION_MODEL), "gpt-4o-mini-transcribe");
                let catalog = |app: &mut App, payload: Value| {
                    let mut payload = payload;
                    payload["generation"] = app.models_generation().into();
                    events_tx.send((events::MODEL_CATALOG.into(), payload)).unwrap();
                    SendMessageW(app.host, EVENTS, None, None);
                };
                app.command(api::REFRESH, BN_CLICKED);
                assert!(!IsWindowEnabled(app.control(api::REFRESH)).as_bool());
                catalog(&mut app, serde_json::json!({
                    "transcription": ["canary-1b-v2", "whisper-1"], "chat": ["gpt-4.1-nano", "text-light"]
                }));
                assert_eq!(SendMessageW(app.control(api::TRANSCRIPTION_MODEL), CB_GETCOUNT, None, None).0, 3);
                assert_eq!(app.text(api::TRANSCRIPTION_MODEL), "gpt-4o-mini-transcribe (not advertised)");
                pick(&mut app, api::TRANSCRIPTION_MODEL, 2);
                assert_eq!(setting(&app, "transcription_model"), "whisper-1");
                assert_eq!(SendMessageW(app.control(api::CLEANUP_MODEL), CB_GETCOUNT, None, None).0, 2);
                pick(&mut app, api::CLEANUP_MODEL, 1);
                assert_eq!(setting(&app, "llm_model"), "text-light");
                // The lists stay across tabs without asking again.
                app.command(TAB + GENERAL, BN_CLICKED);
                app.command(TAB + DICTATION, BN_CLICKED);
                assert_eq!(SendMessageW(app.control(api::TRANSCRIPTION_MODEL), CB_GETCOUNT, None, None).0, 2);
                assert_eq!(app.text(api::TRANSCRIPTION_MODEL), "whisper-1");
                app.command(api::REFRESH, BN_CLICKED);
                catalog(&mut app, serde_json::json!({"error": "unauthorized"}));
                assert!(app.status.contains("unauthorized"));
                assert_eq!(app.text(api::TRANSCRIPTION_MODEL), "whisper-1");
                // Presets replace typed numbers.
                assert_eq!(app.text(RECORDING_LIMIT), "5 minutes");
                pick(&mut app, RECORDING_LIMIT, 1);
                assert_eq!(setting(&app, "recording_limit_seconds"), "120");
                pick(&mut app, PASTE_DELAY, 4);
                assert_eq!(setting(&app, "paste_delay_ms"), "500");
                app.runtime.set_setting("paste_delay_ms", "75").unwrap();
                app.drafts.insert("paste_delay_ms".into(), "75".into());
                app.render();
                assert_eq!(app.text(PASTE_DELAY), "75 ms");
                // Cleanup off disables its model.
                SendMessageW(app.control(103), BM_SETCHECK, Some(WPARAM(0)), None);
                app.command(103, BN_CLICKED);
                assert!(!IsWindowEnabled(app.control(api::CLEANUP_MODEL)).as_bool());

                // Voice.
                let speech = serde_json::json!([
                    {"id":"qwen3-tts","name":"Qwen","capabilities":{
                        "modes":{"speaker":["worker"]},"voice_fields":{"speaker":["voice","voice_instruction"]},
                        "voices":[{"id":18,"name":"Test speaker"}],
                        "engine_options":{"temperature":"number","num_step":"integer","stream":"boolean"}
                    }},
                    {"id":"omnivoice","name":"Omni","capabilities":{
                        "modes":{"auto":["worker"]},"voice_fields":{"auto":[]},"engine_options":{}
                    }}
                ]);
                app.page = VOICE;
                app.catalog_loading = true;
                app.render();
                assert!(!IsWindowEnabled(app.control(610)).as_bool());
                assert_eq!(app.text(624), "Open audio folder");
                assert_eq!(setting(&app, "tts_virtual_mic"), "false");
                SendMessageW(app.control(625), BM_SETCHECK, Some(WPARAM(1)), None);
                app.command(625, BN_CLICKED);
                assert_eq!(setting(&app, "tts_virtual_mic"), "true");
                let discovery = |app: &mut App, models: Value| {
                    events_tx.send((events::SPEECH_CATALOG.into(), serde_json::json!({
                        "generation": app.catalog_generation, "models": models
                    }))).unwrap();
                    SendMessageW(app.host, EVENTS, None, None);
                };
                discovery(&mut app, speech.clone());
                assert!(!app.catalog_loading);
                assert!(IsWindowEnabled(app.control(610)).as_bool());
                assert_eq!(SendMessageW(app.control(610), CB_GETCOUNT, None, None).0, 3);
                assert_eq!(SendMessageW(app.control(610), CB_GETCURSEL, None, None).0, 1);
                assert_eq!(setting(&app, "tts_model"), "qwen3-tts");
                assert_eq!(setting(&app, "tts_voice:qwen3-tts"), "18");
                pick(&mut app, 611, 0);
                assert_eq!(setting(&app, "tts_voice:qwen3-tts"), "");
                discovery(&mut app, serde_json::json!([]));
                assert!(app.status.contains("no speech models"));
                assert!(!IsWindowEnabled(app.control(610)).as_bool());
                discovery(&mut app, speech);
                pick(&mut app, 611, 1);
                assert_eq!(setting(&app, "tts_voice:qwen3-tts"), "18");
                app.label_text(620, "Softly");
                app.command(620, EN_KILLFOCUS);
                assert_eq!(setting(&app, "tts_voice_instruction:qwen3-tts"), "Softly");
                // Engine options are typed controls, sorted by name: num_step, stream, temperature.
                assert_eq!(app.engine_fields.len(), 3);
                app.label_text(650, "1.5");
                app.command(650, EN_KILLFOCUS);
                assert!(app.status.contains("num_step must be integer"));
                assert_eq!(setting(&app, "tts_engine_options"), "{}");
                app.label_text(650, "8");
                app.label_text(652, "0.8");
                app.command(652, EN_KILLFOCUS);
                pick(&mut app, 651, 2);
                assert_eq!(
                    setting(&app, "tts_engine_options"),
                    r#"{"qwen3-tts":{"num_step":8,"stream":false,"temperature":0.8}}"#
                );
                pick(&mut app, 610, 2);
                assert!(!IsWindowEnabled(app.control(611)).as_bool());
                assert!(!IsWindowEnabled(app.control(620)).as_bool());
                assert!(app.engine_fields.is_empty());
                pick(&mut app, 610, 1);
                assert_eq!(app.text(620), "Softly");
                assert_eq!(app.text(652), "0.8");

                // Shortcuts: Reset restores the dictation default, Clear disables a voice one.
                app.command(TAB + SHORTCUTS, BN_CLICKED);
                app.runtime.set_setting("hotkey", "Ctrl+Alt+F9").unwrap();
                app.command(640, BN_CLICKED);
                assert_eq!(setting(&app, "hotkey"), "Ctrl+Win");
                assert_eq!(app.text(630), "Ctrl+Win");
                app.persist("tts_hotkey_sample_mic", "Ctrl+Alt+F8");
                app.command(641, BN_CLICKED);
                assert_eq!(setting(&app, "tts_hotkey_sample_mic"), "");

                // Languages.
                app.command(CHOOSE_LANGUAGES, BN_CLICKED);
                app.fill_languages("French");
                assert_eq!(app.languages, ["fr"]);
                assert_eq!(SendMessageW(app.control(500), LB_GETSEL, Some(WPARAM(0)), None).0, 1);
                SendMessageW(app.control(500), LB_SETSEL, Some(WPARAM(0)), Some(LPARAM(0)));
                app.command(500, LBN_SELCHANGE);
                assert_eq!(setting(&app, "languages"), r#"["en"]"#);
                app.command(LANGUAGES_DONE, BN_CLICKED);
                assert_eq!(app.page, GENERAL);

                let old = app.window;
                SendMessageW(old, WM_CLOSE, None, None);
                assert!(!IsWindow(Some(old)).as_bool());
                assert!(app.window.0.is_null());
                assert!(app.children.is_empty());
                assert!(app.font.0.is_null());
                // Model lists outlive the window.
                assert_eq!(app.speech_models.len(), 2);
                SendMessageW(app.host, TRAY, Some(WPARAM(0)), Some(LPARAM(WM_LBUTTONUP as isize)));
                let mut message = MSG::default();
                assert!(PeekMessageW(&mut message, Some(app.host), OPEN, OPEN, PM_REMOVE).as_bool());
                DispatchMessageW(&message);
                assert!(!app.window.0.is_null());
                SendMessageW(app.window, WM_CLOSE, None, None);
                SetWindowLongPtrW(app.host, GWLP_USERDATA, 0);
                let _ = DestroyWindow(app.host);
                let _ = DeleteObject(app.background.into());
                let _ = DeleteObject(app.input_background.into());
            }
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}
