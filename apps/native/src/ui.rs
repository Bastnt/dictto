use dictto_native::runtime::NativeRuntime;
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
#[path = "ui_models.rs"]
mod models;

const TRAY: u32 = WM_APP + 42;
const EVENTS: u32 = WM_APP + 43;
const OPEN: u32 = WM_APP + 44;
const STATUS: usize = 90;
const CHECKS: &[(usize, &str)] = &[
    (100, "auto_start"),
    (101, "sound_effects_enabled"),
    (102, "transcription_warmup"),
    (103, "cleanup_enabled"),
];
const FIELDS: &[(usize, &str)] = &[
    (200, "hotkey"),
    (201, "api_base_url"),
    (202, "transcription_model"),
    (203, "llm_model"),
    (204, "recording_limit_seconds"),
    (205, "paste_delay_ms"),
];

struct App {
    runtime: NativeRuntime,
    events: mpsc::Receiver<(String, Value)>,
    host: HWND,
    window: HWND,
    children: Vec<(usize, HWND)>,
    drafts: HashMap<String, String>,
    page: usize,
    font: HFONT,
    background: HBRUSH,
    input_background: HBRUSH,
    dark: bool,
    dpi: u32,
    icon: HICON,
    taskbar_message: u32,
    microphones: Vec<String>,
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
    unsafe fn key_hint(&self) {
        let hint = self
            .runtime
            .key_hint()
            .unwrap_or_else(|_| "Enter API key".into());
        SendMessageW(
            self.control(210),
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(wide(&hint).as_ptr() as isize)),
        );
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
    unsafe fn edit(&mut self, id: usize, key: &str, rect: [i32; 4]) {
        let value = self.drafts.get(key).cloned().unwrap_or_default();
        self.add(
            id,
            w!("EDIT"),
            &value,
            WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            rect,
        );
    }
    unsafe fn capture_drafts(&mut self) {
        for &(id, key) in FIELDS {
            if !self.control(id).0.is_null() {
                self.drafts.insert(key.into(), self.text(id));
            }
        }
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
    unsafe fn show(&mut self) -> Result<(), String> {
        if !self.window.0.is_null() {
            let _ = ShowWindow(self.window, SW_RESTORE);
            let _ = SetForegroundWindow(self.window);
            return Ok(());
        }
        self.drafts = self.runtime.get_settings()?;
        self.page = 0;
        self.dpi = GetDpiForSystem();
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: self.scale(660),
            bottom: self.scale(440),
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
            w!("Dictto Native"),
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
        for (_, hwnd) in self.children.drain(..) {
            let _ = DestroyWindow(hwnd);
        }
        self.label("Settings", [20, 16, 220, 28]);
        for (id, text) in [(10, "General"), (11, "Audio"), (12, "API")] {
            self.button(id, text, [350 + (id as i32 - 10) * 96, 12, 88, 30]);
        }
        match self.page {
            0 => {
                for (id, y, title, description) in [
                    (100, 66, "Start on boot", "Launch Dictto when you sign in"),
                    (101, 124, "Sound effects", "Play sounds on recording events"),
                ] {
                    self.label(title, [20, y, 300, 22]);
                    self.label(description, [20, y + 23, 390, 20]);
                    self.check(
                        id,
                        "Enabled",
                        self.drafts
                            .get(CHECKS.iter().find(|(i, _)| *i == id).unwrap().1)
                            .map(String::as_str)
                            == Some("true"),
                        [530, y + 5, 105, 28],
                    );
                }
                self.label("Languages", [20, 188, 155, 22]);
                let languages: Vec<String> = serde_json::from_str(
                    self.drafts
                        .get("languages")
                        .map(String::as_str)
                        .unwrap_or("[]"),
                )
                .unwrap_or_default();
                let summary = if languages.is_empty() || languages.iter().any(|v| v == "auto") {
                    "Auto-detect".into()
                } else {
                    languages.join(", ")
                };
                self.label(&summary, [180, 188, 340, 22]);
                self.button(13, "Change", [546, 183, 88, 30]);
                self.label("Push-to-talk hotkey", [20, 245, 250, 22]);
                self.edit(200, "hotkey", [315, 241, 225, 28]);
                self.button(300, "Save", [550, 240, 84, 30]);
                self.label(
                    "Hold to dictate; double-press for continuous recording.",
                    [20, 282, 590, 22],
                );
            }
            1 => {
                self.label("Microphone", [20, 65, 200, 22]);
                self.microphones = vec!["auto-detect".into()];
                match self.runtime.microphones() {
                    Ok(items) => self.microphones.extend(items.into_iter().map(|m| m.name)),
                    Err(e) => self.status = e,
                }
                let combo = self.add(
                    400,
                    w!("COMBOBOX"),
                    "",
                    WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                    [200, 61, 434, 250],
                );
                for (index, mic) in self.microphones.iter().enumerate() {
                    let label = if index == 0 {
                        let current = self.runtime.current_microphone();
                        if current.is_empty() {
                            "Auto-detect".into()
                        } else {
                            format!("Auto-detect ({current})")
                        }
                    } else {
                        mic.clone()
                    };
                    SendMessageW(
                        combo,
                        CB_ADDSTRING,
                        None,
                        Some(LPARAM(wide(&label).as_ptr() as isize)),
                    );
                }
                let selected = self
                    .microphones
                    .iter()
                    .position(|m| Some(m) == self.drafts.get("microphone_device"))
                    .unwrap_or(0);
                SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(selected)), None);
                self.button(401, "Refresh", [546, 102, 88, 30]);
                self.label("Recording", [20, 155, 180, 22]);
                for (id, text) in [(410, "Record"), (411, "Stop"), (412, "Cancel")] {
                    self.button(id, text, [350 + (id as i32 - 410) * 96, 150, 88, 30]);
                    let _ = EnableWindow(self.control(id), !self.smoke);
                }
                self.label("Recording limit (seconds)", [20, 220, 275, 22]);
                self.edit(204, "recording_limit_seconds", [350, 216, 180, 28]);
                self.button(304, "Save", [546, 215, 88, 30]);
                self.label("Paste delay (milliseconds)", [20, 274, 275, 22]);
                self.edit(205, "paste_delay_ms", [350, 270, 180, 28]);
                self.button(305, "Save", [546, 269, 88, 30]);
            }
            2 => {
                self.label("API base URL", [20, 62, 285, 22]);
                self.edit(201, "api_base_url", [20, 88, 300, 28]);
                self.label("API key", [342, 62, 290, 22]);
                self.add(
                    210,
                    w!("EDIT"),
                    "",
                    WS_TABSTOP | WS_BORDER | WINDOW_STYLE((ES_AUTOHSCROLL | ES_PASSWORD) as u32),
                    [342, 88, 292, 28],
                );
                SendMessageW(
                    self.control(210),
                    EM_SETCUEBANNER,
                    Some(WPARAM(1)),
                    Some(LPARAM(
                        wide(
                            &self
                                .runtime
                                .key_hint()
                                .unwrap_or_else(|_| "Enter API key".into()),
                        )
                        .as_ptr() as isize,
                    )),
                );
                self.button(301, "Save URL", [20, 124, 100, 28]);
                self.button(310, "Save key", [342, 124, 100, 28]);
                self.button(311, "Remove key", [452, 124, 110, 28]);
                self.label("Transcription model", [20, 171, 280, 22]);
                self.check(
                    102,
                    "Warm transcription model",
                    self.drafts.get("transcription_warmup").map(String::as_str) == Some("true"),
                    [395, 166, 240, 28],
                );
                self.edit(202, "transcription_model", [20, 200, 614, 28]);
                self.label(
                    "Warm-up sends 1 second of silence; provider charges may apply.",
                    [20, 235, 614, 22],
                );
                self.label("Cleanup model", [20, 275, 280, 22]);
                self.check(
                    103,
                    "Text cleanup",
                    self.drafts.get("cleanup_enabled").map(String::as_str) == Some("true"),
                    [485, 270, 150, 28],
                );
                self.edit(203, "llm_model", [20, 304, 614, 28]);
                let _ = EnableWindow(
                    self.control(203),
                    self.drafts.get("cleanup_enabled").map(String::as_str) == Some("true"),
                );
                self.button(320, "Save models", [510, 346, 124, 30]);
                self.label(
                    "When cleanup is off, paste the original transcript.",
                    [20, 349, 475, 22],
                );
            }
            _ => {
                self.label("Languages (select all that apply)", [20, 63, 410, 22]);
                self.button(10, "Done", [546, 60, 88, 28]);
                let values: Vec<String> = serde_json::from_str(
                    self.drafts
                        .get("languages")
                        .map(String::as_str)
                        .unwrap_or("[]"),
                )
                .unwrap_or_default();
                self.check(
                    501,
                    "Auto-detect",
                    values.is_empty() || values.iter().any(|v| v == "auto"),
                    [20, 96, 180, 28],
                );
                self.label("Search", [220, 99, 65, 22]);
                self.add(
                    502,
                    w!("EDIT"),
                    "",
                    WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    [290, 96, 344, 28],
                );
                self.add(
                    500,
                    w!("LISTBOX"),
                    "",
                    WS_TABSTOP
                        | WS_BORDER
                        | WS_VSCROLL
                        | WINDOW_STYLE(
                            (LBS_MULTIPLESEL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
                        ),
                    [20, 139, 614, 245],
                );
                self.fill_languages("");
            }
        }
        self.add(
            STATUS,
            w!("STATIC"),
            &self.status.clone(),
            WINDOW_STYLE::default(),
            [20, 400, 614, 36],
        );
        let _ = InvalidateRect(Some(self.window), None, true);
    }
    unsafe fn fill_languages(&mut self, filter: &str) {
        let list = self.control(500);
        SendMessageW(list, LB_RESETCONTENT, None, None);
        self.languages.clear();
        let selected: Vec<String> = serde_json::from_str(
            self.drafts
                .get("languages")
                .map(String::as_str)
                .unwrap_or("[]"),
        )
        .unwrap_or_default();
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
        let result = if (key == "recording_limit_seconds"
            && value.parse::<u64>().map(|v| v == 0).unwrap_or(true))
            || (key == "paste_delay_ms" && value.parse::<u64>().is_err())
        {
            Err("Enter a valid whole number; recording limit must be positive.".into())
        } else {
            self.runtime.set_setting(key, value)
        };
        match result {
            Ok(()) => {
                self.drafts.insert(key.into(), value.into());
                self.status = "Saved".into();
                true
            }
            Err(e) => {
                self.status = e;
                false
            }
        }
    }
    unsafe fn command(&mut self, id: usize, notification: u32) {
        if id == 200 && notification == EN_SETFOCUS {
            self.runtime.set_hotkey_capture(true);
            return;
        }
        if id == 200 && notification == EN_KILLFOCUS {
            self.runtime.set_hotkey_capture(false);
            return;
        }
        if id == 502 && notification == EN_CHANGE {
            self.fill_languages(&self.text(502));
            return;
        }
        if id == 400 && notification == CBN_SELCHANGE {
            let index = SendMessageW(self.control(400), CB_GETCURSEL, None, None).0 as usize;
            if let Some(value) = self.microphones.get(index).cloned() {
                self.persist("microphone_device", &value);
            }
        } else if id == 500 && notification == LBN_SELCHANGE {
            let mut values: Vec<String> = serde_json::from_str(
                self.drafts
                    .get("languages")
                    .map(String::as_str)
                    .unwrap_or("[]"),
            )
            .unwrap_or_default();
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
            self.capture_drafts();
            match id {
                10..=13 => {
                    self.page = if id == 13 { 3 } else { id - 10 };
                    self.render();
                    let _ = SetFocus(Some(self.control(if id == 13 { 10 } else { id })));
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
                            self.control(203),
                            self.drafts.get(key).map(String::as_str) == Some("true"),
                        );
                    }
                }
                300..=305 => {
                    let field = id - 100;
                    if let Some((_, key)) = FIELDS.iter().find(|(i, _)| *i == field) {
                        self.persist(key, &self.text(field));
                    }
                }
                310 => {
                    let key = self.text(210);
                    let result = if key.trim().is_empty() {
                        Err("Enter an API key before saving".into())
                    } else {
                        self.runtime.save_key(key.trim())
                    };
                    self.status = match result {
                        Ok(()) => {
                            self.label_text(210, "");
                            self.key_hint();
                            "API key saved in Windows Credential Manager".into()
                        }
                        Err(e) => e,
                    };
                }
                311 => {
                    if MessageBoxW(
                        Some(self.window),
                        w!("Remove the saved API key?"),
                        w!("Dictto"),
                        MB_YESNO | MB_ICONQUESTION,
                    ) == IDYES
                    {
                        self.status = match self.runtime.remove_key() {
                            Ok(()) => {
                                self.key_hint();
                                "API key removed".into()
                            }
                            Err(e) => e,
                        };
                    }
                }
                320 => {
                    if self.persist("transcription_model", &self.text(202)) {
                        self.persist("llm_model", &self.text(203));
                    }
                }
                401 => self.render(),
                410 if !self.smoke => self.runtime.start_recording(),
                411 if !self.smoke => self.runtime.stop_recording(),
                412 if !self.smoke => self.runtime.cancel_recording(),
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
            if name == "audio-devices-changed" {
                devices_changed = true;
            }
            if matches!(
                name.as_str(),
                "error" | "recording-error" | "dictation-error"
            ) {
                // The shared error event may contain provider details; avoid displaying secrets.
                self.status = "Dictation failed. Check the microphone and API settings.".into();
            } else if matches!(
                name.as_str(),
                "recording-state" | "recording-state-changed" | "processing-state"
            ) {
                self.status = payload["state"]
                    .as_str()
                    .or_else(|| payload.as_str())
                    .unwrap_or("")
                    .into();
            }
        }
        if devices_changed && self.page == 1 && !self.window.0.is_null() {
            self.capture_drafts();
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
            (*pointer).drafts.clear();
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
            let input = message == WM_CTLCOLOREDIT || message == WM_CTLCOLORLISTBOX;
            SetTextColor(dc, COLORREF(if dark { 0xf0f0f0 } else { 0x202020 }));
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
            (*pointer).capture_drafts();
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
    let runtime = NativeRuntime::new(data, move |name, payload| {
        if let Some(overlay) = sink_overlay.lock().unwrap().as_ref() {
            overlay.send(name, payload.clone());
        }
        if matches!(
            name,
            "error"
                | "recording-error"
                | "dictation-error"
                | "recording-state"
                | "recording-state-changed"
                | "processing-state"
                | "audio-devices-changed"
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
    runtime: NativeRuntime,
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
    let mut app = Box::new(App {
        runtime,
        events,
        host: HWND::default(),
        window: HWND::default(),
        children: Vec::new(),
        drafts: HashMap::new(),
        page: 0,
        font: HFONT::default(),
        background: HBRUSH::default(),
        input_background: HBRUSH::default(),
        dark: false,
        dpi: 96,
        icon,
        taskbar_message: RegisterWindowMessageW(w!("TaskbarCreated")),
        microphones: Vec::new(),
        languages: Vec::new(),
        smoke,
        status: if smoke {
            "Preview: recording services disabled".into()
        } else {
            String::new()
        },
    });
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
            if app.window.0.is_null() || !IsDialogMessageW(app.window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    })();
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
            let runtime = NativeRuntime::new(path.clone(), |_, _| {}).unwrap();
            runtime.set_setting("languages", r#"["fr","en"]"#).unwrap();
            let (_, events) = mpsc::channel();
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
                let mut app = Box::new(App {
                    runtime,
                    events,
                    host: HWND::default(),
                    window: HWND::default(),
                    children: Vec::new(),
                    drafts: HashMap::new(),
                    page: 0,
                    font: HFONT::default(),
                    background: HBRUSH::default(),
                    input_background: HBRUSH::default(),
                    dark: false,
                    dpi: 96,
                    icon: HICON::default(),
                    taskbar_message: 0,
                    microphones: Vec::new(),
                    languages: Vec::new(),
                    smoke: true,
                    status: String::new(),
                });
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
                app.command(12, BN_CLICKED);
                assert_eq!(app.text(210), "");
                app.label_text(201, "http://localhost:1234/v1");
                app.command(301, BN_CLICKED);
                assert_eq!(
                    app.runtime.get_settings().unwrap()["api_base_url"],
                    "http://localhost:1234/v1"
                );
                app.label_text(202, "draft-model");
                app.command(10, BN_CLICKED);
                app.command(12, BN_CLICKED);
                assert_eq!(app.text(202), "draft-model");
                app.command(320, BN_CLICKED);
                assert_eq!(
                    app.runtime.get_settings().unwrap()["transcription_model"],
                    "draft-model"
                );
                app.command(13, BN_CLICKED);
                app.fill_languages("French");
                assert_eq!(app.languages, ["fr"]);
                assert_eq!(
                    SendMessageW(app.control(500), LB_GETSEL, Some(WPARAM(0)), None).0,
                    1
                );
                SendMessageW(
                    app.control(500),
                    LB_SETSEL,
                    Some(WPARAM(0)),
                    Some(LPARAM(0)),
                );
                app.command(500, LBN_SELCHANGE);
                assert_eq!(
                    app.runtime.get_settings().unwrap()["languages"],
                    r#"["en"]"#
                );
                app.command(11, BN_CLICKED);
                app.label_text(204, "0");
                app.command(304, BN_CLICKED);
                assert_ne!(
                    app.runtime.get_settings().unwrap()["recording_limit_seconds"],
                    "0"
                );
                assert!(app.status.contains("positive"));
                let old = app.window;
                SendMessageW(old, WM_CLOSE, None, None);
                assert!(!IsWindow(Some(old)).as_bool());
                assert!(app.window.0.is_null());
                assert!(app.children.is_empty());
                assert!(app.font.0.is_null());
                SendMessageW(
                    app.host,
                    TRAY,
                    Some(WPARAM(0)),
                    Some(LPARAM(WM_LBUTTONUP as isize)),
                );
                let mut message = MSG::default();
                assert!(
                    PeekMessageW(&mut message, Some(app.host), OPEN, OPEN, PM_REMOVE).as_bool()
                );
                DispatchMessageW(&message);
                assert!(!app.window.0.is_null());
                assert_eq!(app.text(200), app.runtime.get_settings().unwrap()["hotkey"]);
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
