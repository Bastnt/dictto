use serde_json::Value;
use std::sync::{mpsc, Arc};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const UPDATE: u32 = WM_APP + 10;
const NORMAL_WIDTH: i32 = 216;
const CONTINUOUS_WIDTH: i32 = 340;
const ERROR_WIDTH: i32 = 340;
const HEIGHT: i32 = 44;
type Action = Arc<dyn Fn() + Send + Sync>;

enum Message {
    Event(String, Value),
    Quit,
}

#[derive(Clone)]
pub struct NativeOverlay {
    hwnd: isize,
    sender: mpsc::Sender<Message>,
}

impl NativeOverlay {
    pub fn spawn(stop: Action, cancel: Action) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let (ready, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("native-overlay".into())
            .spawn(move || unsafe {
                let instance = match GetModuleHandleW(None) {
                    Ok(value) => HINSTANCE(value.0),
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    lpszClassName: w!("Dictto.Native.RecordingOverlay"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let mut state = Box::new(State {
                    receiver,
                    stop,
                    cancel,
                    recording: false,
                    continuous: false,
                    processing: false,
                    error: String::new(),
                    bars: vec![0.05; 24],
                    animation_phase: 0,
                });
                let state_pointer = (&mut *state as *mut State).cast();
                let created = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                    class.lpszClassName,
                    w!("Dictto Native Recording"),
                    WS_POPUP,
                    0,
                    0,
                    NORMAL_WIDTH,
                    HEIGHT,
                    None,
                    None,
                    Some(instance),
                    Some(state_pointer),
                );
                let hwnd = match created {
                    Ok(hwnd) => hwnd,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                if let Err(error) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_COLORKEY)
                {
                    let _ = ready.send(Err(error.to_string()));
                    let _ = DestroyWindow(hwnd);
                    return;
                }
                let _ = ready.send(Ok(hwnd.0 as isize));
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            hwnd: result.recv().map_err(|error| error.to_string())??,
            sender,
        })
    }

    pub fn send(&self, event: &str, payload: Value) {
        if !matches!(
            event,
            "recording-state-changed" | "waveform-data" | "dictation-error"
        ) {
            return;
        }
        if self
            .sender
            .send(Message::Event(event.into(), payload))
            .is_ok()
        {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as _)), UPDATE, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(Message::Quit);
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as _)), UPDATE, WPARAM(0), LPARAM(0));
        }
    }
}

struct State {
    receiver: mpsc::Receiver<Message>,
    stop: Action,
    cancel: Action,
    recording: bool,
    continuous: bool,
    processing: bool,
    error: String,
    bars: Vec<f32>,
    animation_phase: u8,
}

impl State {
    fn event(&mut self, name: &str, value: &Value) {
        match name {
            "recording-state-changed" => {
                self.recording = value["state"] == "recording";
                self.processing = value["state"] == "processing";
                self.continuous = self.recording && value["mode"] == "continuous";
                if self.recording {
                    self.error.clear();
                }
            }
            "waveform-data" => {
                if let Some(values) = value["amplitudes"].as_array() {
                    let samples: Vec<f32> = values
                        .iter()
                        .map(|n| n.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32)
                        .collect();
                    self.bars = fit_bars(&samples, 24);
                }
            }
            "dictation-error" => {
                self.recording = false;
                self.processing = false;
                self.continuous = false;
                self.error = dictto_native::services::transcription::friendly_error(
                    value["message"].as_str().unwrap_or_default()).into()
            }
            _ => {}
        }
    }

    fn active(&self) -> bool {
        self.recording || self.processing || !self.error.is_empty()
    }

    fn size(&self) -> (i32, i32) {
        if !self.error.is_empty() {
            (ERROR_WIDTH, 80)
        } else if self.continuous {
            (CONTINUOUS_WIDTH, HEIGHT)
        } else {
            (NORMAL_WIDTH, HEIGHT)
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wp, lp);
    }
    let state = &mut *pointer;
    match message {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        UPDATE => {
            let mut layout_changed = false;
            while let Ok(update) = state.receiver.try_recv() {
                match update {
                    Message::Quit => {
                        let _ = DestroyWindow(hwnd);
                        return LRESULT(0);
                    }
                    Message::Event(name, value) => {
                        if name == "dictation-error" {
                            SetTimer(Some(hwnd), 1, 15000, None);
                        }
                        if name == "recording-state-changed" && value["state"] == "recording" {
                            let _ = KillTimer(Some(hwnd), 1);
                        }
                        layout_changed |= name != "waveform-data";
                        state.event(&name, &value);
                    }
                }
            }
            if layout_changed {
                if state.active() {
                    place(hwnd, state);
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            if state.processing {
                let _ = SetTimer(Some(hwnd), 2, 200, None);
            } else {
                let _ = KillTimer(Some(hwnd), 2);
            }
            if state.active() {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            if wp.0 == 2 {
                state.animation_phase = (state.animation_phase + 1) % 8;
                let _ = InvalidateRect(Some(hwnd), None, false);
                return LRESULT(0);
            }
            let _ = KillTimer(Some(hwnd), 1);
            state.error.clear();
            if !state.active() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN if state.continuous && state.error.is_empty() => {
            let scale = GetDpiForWindow(hwnd).max(96) as f64 / 96.0;
            let x = (lp.0 as u16 as i16) as f64 / scale;
            let (width, _) = state.size();
            if x < 80.0 {
                (state.cancel)();
            } else if x > (width - 80) as f64 {
                (state.stop)();
            }
            return LRESULT(0);
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            place(hwnd, state);
            return LRESULT(0);
        }
        WM_PAINT => {
            paint(hwnd, state);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wp, lp)
}

unsafe fn place(hwnd: HWND, state: &State) {
    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
        return;
    }
    let scale = GetDpiForWindow(hwnd).max(96) as f64 / 96.0;
    let (logical_width, logical_height) = state.size();
    let width = (logical_width as f64 * scale) as i32;
    let height = (logical_height as f64 * scale) as i32;
    let area = info.rcWork;
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        area.left + (area.right - area.left - width) / 2,
        area.bottom - height - (18.0 * scale) as i32,
        width,
        height,
        SWP_NOACTIVATE,
    );
}

fn fit_bars(samples: &[f32], count: usize) -> Vec<f32> {
    match samples.len() {
        0 => vec![0.05; count],
        1 => vec![samples[0]; count],
        length => (0..count)
            .map(|index| {
                let position = index as f32 * (length - 1) as f32 / (count - 1).max(1) as f32;
                let lower = position.floor() as usize;
                let upper = (lower + 1).min(length - 1);
                let blend = position - lower as f32;
                samples[lower] * (1.0 - blend) + samples[upper] * blend
            })
            .collect(),
    }
}

fn dark_theme() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .unwrap_or(0)
        == 0
}

unsafe fn paint(hwnd: HWND, state: &State) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut bounds = RECT::default();
    let _ = GetClientRect(hwnd, &mut bounds);
    let black = CreateSolidBrush(COLORREF(0));
    FillRect(dc, &bounds, black);
    let _ = DeleteObject(black.into());
    let dark = dark_theme();
    let fill = CreateSolidBrush(COLORREF(if dark { 0x00262626 } else { 0x00f4f4f4 }));
    let old = SelectObject(dc, fill.into());
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let (logical_width, _) = state.size();
    let scale = (bounds.right as f64 / logical_width as f64).max(1.0);
    let radius = (20.0 * scale) as i32;
    let _ = RoundRect(
        dc,
        1,
        1,
        bounds.right - 1,
        bounds.bottom - 1,
        radius,
        radius,
    );
    let _ = SelectObject(dc, old);
    let _ = SelectObject(dc, old_pen);
    let _ = DeleteObject(fill.into());
    let foreground = COLORREF(if dark { 0x00f0f0f0 } else { 0x00202020 });
    let _ = SetTextColor(dc, foreground);
    let _ = SetBkMode(dc, TRANSPARENT);
    let mut description = LOGFONTW {
        lfHeight: -(14.0 * scale) as i32,
        ..Default::default()
    };
    for (slot, character) in description
        .lfFaceName
        .iter_mut()
        .zip("Segoe UI".encode_utf16())
    {
        *slot = character;
    }
    let font = CreateFontIndirectW(&description);
    let previous_font = SelectObject(dc, font.into());
    let text = state.error.as_str();
    if state.processing && state.error.is_empty() {
        let center_x = bounds.right / 2;
        let center_y = bounds.bottom / 2;
        let radius = (8.0 * scale) as i32;
        for dot in 0..8 {
            let angle =
                (dot as f64 - f64::from(state.animation_phase)) * std::f64::consts::PI / 4.0;
            let x = center_x + (radius as f64 * angle.cos()) as i32;
            let y = center_y + (radius as f64 * angle.sin()) as i32;
            let shade = (dot + usize::from(state.animation_phase)) % 8;
            let channel = if dark {
                0x4c + shade as u32 * 0x19
            } else {
                0xf2 - shade as u32 * 0x19
            };
            let color = COLORREF((channel << 16) | (channel << 8) | channel);
            let brush = CreateSolidBrush(color);
            let diameter = (4.0 * scale) as i32;
            let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
            let old_brush = SelectObject(dc, brush.into());
            let _ = Ellipse(
                dc,
                x - diameter / 2,
                y - diameter / 2,
                x + diameter / 2,
                y + diameter / 2,
            );
            let _ = SelectObject(dc, old_brush);
            let _ = SelectObject(dc, old_pen);
            let _ = DeleteObject(brush.into());
        }
    } else if !text.is_empty() {
        let mut rect = RECT {
            left: 12,
            top: 4,
            right: bounds.right - 12,
            bottom: bounds.bottom - 4,
        };
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(
            dc,
            &mut wide,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_WORDBREAK,
        );
    } else if state.recording {
        let brush = CreateSolidBrush(foreground);
        let count = state.bars.len().min(24);
        let spacing = 5.0;
        let start_x = logical_width as f64 / 2.0 - (count.saturating_sub(1) as f64 * spacing / 2.0);
        for (i, amplitude) in state.bars.iter().take(count).enumerate() {
            let x = ((start_x + i as f64 * spacing) * scale) as i32;
            let height = ((3.0 + f64::from(*amplitude) * 19.0) * scale) as i32;
            FillRect(
                dc,
                &RECT {
                    left: x,
                    top: (bounds.bottom - height) / 2,
                    right: x + (2.0 * scale) as i32,
                    bottom: (bounds.bottom + height) / 2,
                },
                brush,
            );
        }
        let _ = DeleteObject(brush.into());
        if state.continuous {
            for (text, left, right) in [
                ("Cancel", 7, 70),
                ("Stop", logical_width - 70, logical_width - 7),
            ] {
                let mut rect = RECT {
                    left: (left as f64 * scale) as i32,
                    top: 0,
                    right: (right as f64 * scale) as i32,
                    bottom: bounds.bottom,
                };
                let mut wide: Vec<u16> = text.encode_utf16().collect();
                DrawTextW(
                    dc,
                    &mut wide,
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            }
        }
    }
    let _ = SelectObject(dc, previous_font);
    let _ = DeleteObject(font.into());
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_window_shows_without_activation_and_routes_controls() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use windows::Win32::Foundation::*;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let stopped = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let cancel = cancelled.clone();
        let overlay = super::NativeOverlay::spawn(
            Arc::new(move || {
                stop.store(true, Ordering::Release);
            }),
            Arc::new(move || {
                cancel.store(true, Ordering::Release);
            }),
        )
        .unwrap();
        let hwnd = HWND(overlay.hwnd as _);
        overlay.send(
            "recording-state-changed",
            serde_json::json!({"state":"recording", "mode":"continuous"}),
        );
        let wait = |condition: &dyn Fn() -> bool| {
            for _ in 0..100 {
                if condition() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(condition());
        };
        wait(&|| unsafe { IsWindowVisible(hwnd).as_bool() });
        unsafe {
            assert_ne!(GetForegroundWindow(), hwnd);
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE.0 as isize,
                0
            );
            assert_eq!(
                SendMessageW(hwnd, WM_MOUSEACTIVATE, None, None).0,
                MA_NOACTIVATE as isize
            );
            let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(0), LPARAM(10));
        }
        wait(&|| cancelled.load(Ordering::Acquire));
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect).unwrap();
            let _ = PostMessageW(
                Some(hwnd),
                WM_LBUTTONDOWN,
                WPARAM(0),
                LPARAM((rect.right - 10) as isize),
            );
        }
        wait(&|| stopped.load(Ordering::Acquire));
        assert_ne!(unsafe { GetForegroundWindow() }, hwnd);
        overlay.shutdown();
        wait(&|| !unsafe { IsWindow(Some(hwnd)).as_bool() });
    }

    #[test]
    fn state_clears_errors_when_a_new_recording_begins() {
        let (_, receiver) = std::sync::mpsc::channel();
        let mut state = super::State {
            receiver,
            stop: std::sync::Arc::new(|| {}),
            cancel: std::sync::Arc::new(|| {}),
            recording: false,
            continuous: false,
            processing: false,
            error: String::new(),
            bars: vec![],
            animation_phase: 0,
        };
        state.event(
            "dictation-error",
            &serde_json::json!({"message":"secret backend failure"}),
        );
        assert!(state.active());
        assert!(!state.error.contains("secret"));
        state.event("dictation-error", &serde_json::json!({"message":
            "Canary requires an explicit supported language, such as fr or en"}));
        assert!(state.error.contains("auto-detection"), "{}", state.error);
        assert!(state.error.contains("Languages"));
        state.event(
            "recording-state-changed",
            &serde_json::json!({"state":"recording","mode":"continuous"}),
        );
        assert!(state.error.is_empty());
        assert!(state.recording && state.continuous);
        state.event(
            "recording-state-changed",
            &serde_json::json!({"state":"idle"}),
        );
        assert!(!state.active());
    }
}
