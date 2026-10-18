#![windows_subsystem = "windows"]
mod overlay;
mod ui;
mod diagnostics;
fn main() {
    let _ = diagnostics::initialize();
    unsafe {
        use windows::Win32::UI::HiDpi::*;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if let Err(error) = ui::run() {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let message: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                None,
                windows::core::PCWSTR(message.as_ptr()),
                windows::core::w!("Dictto Native"),
                MB_ICONERROR,
            );
        }
    }
}
