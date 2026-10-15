use serde_json::Value;
use std::sync::{mpsc, Arc};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const UPDATE: u32 = WM_APP + 10;
const WIDTH: i32 = 400;
const HEIGHT: i32 = 54;
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
                });
                let state_pointer = (&mut *state as *mut State).cast();
                let created = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                    class.lpszClassName,
                    w!("Dictto Native Recording"),
                    WS_POPUP,
                    0,
                    0,
                    WIDTH,
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
                    self.bars = values
                        .iter()
                        .map(|n| n.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32)
                        .collect();
                }
            }
            "dictation-error" => {
                self.error = "Dictation failed. Check API settings and the application log.".into()
            }
            _ => {}
        }
    }

    fn active(&self) -> bool {
        self.recording || self.processing || !self.error.is_empty()
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
                    place(hwnd);
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            if state.active() {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
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
            if x < 55.0 {
                (state.cancel)();
            } else if x > (WIDTH - 55) as f64 {
                (state.stop)();
            }
            return LRESULT(0);
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            place(hwnd);
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

unsafe fn place(hwnd: HWND) {
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
    let width = (WIDTH as f64 * scale) as i32;
    let height = (HEIGHT as f64 * scale) as i32;
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
    let scale = (bounds.right as f64 / WIDTH as f64).max(1.0);
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
    let mut description = LOGFONTW { lfHeight: -(14.0 * scale) as i32, ..Default::default() };
    for (slot, character) in description.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
        *slot = character;
    }
    let font = CreateFontIndirectW(&description);
    let previous_font = SelectObject(dc, font.into());
    let text = if !state.error.is_empty() {
        state.error.as_str()
    } else if state.processing {
        "Transcribing..."
    } else {
        ""
    };
    if !text.is_empty() {
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
        for (i, amplitude) in state.bars.iter().enumerate() {
            let x = ((70.0 + i as f64 * 10.5) * scale) as i32;
            let height = (f64::from(*amplitude).max(0.08) * 28.0 * scale) as i32;
            FillRect(
                dc,
                &RECT {
                    left: x,
                    top: (bounds.bottom - height) / 2,
                    right: x + (3.0 * scale) as i32,
                    bottom: (bounds.bottom + height) / 2,
                },
                brush,
            );
        }
        let _ = DeleteObject(brush.into());
        if state.continuous {
            for (text, left, right) in [("Cancel", 7, 58), ("Stop", WIDTH - 55, WIDTH - 5)] {
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
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        use windows::Win32::Foundation::*;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let stopped = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let cancel = cancelled.clone();
        let overlay = super::NativeOverlay::spawn(Arc::new(move || { stop.store(true, Ordering::Release); }),
            Arc::new(move || { cancel.store(true, Ordering::Release); })).unwrap();
        let hwnd = HWND(overlay.hwnd as _);
        overlay.send("recording-state-changed", serde_json::json!({"state":"recording", "mode":"continuous"}));
        let wait = |condition: &dyn Fn() -> bool| {
            for _ in 0..100 {
                if condition() { return; }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(condition());
        };
        wait(&|| unsafe { IsWindowVisible(hwnd).as_bool() });
        unsafe {
            assert_ne!(GetForegroundWindow(), hwnd);
            assert_ne!(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE.0 as isize, 0);
            assert_eq!(SendMessageW(hwnd, WM_MOUSEACTIVATE, None, None).0, MA_NOACTIVATE as isize);
            let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(0), LPARAM(10));
        }
        wait(&|| cancelled.load(Ordering::Acquire));
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect).unwrap();
            let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(0), LPARAM((rect.right - 10) as isize));
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
        };
        state.event(
            "dictation-error",
            &serde_json::json!({"message":"secret backend failure"}),
        );
        assert!(state.active());
        assert!(!state.error.contains("secret"));
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
