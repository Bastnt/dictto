//! Low-level keyboard hook feeding the dictation state machine.
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode as u16;

        let is_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
        let is_up = wparam.0 == WM_KEYUP as usize || wparam.0 == WM_SYSKEYUP as usize;

        if (is_down || is_up)
            && dictto_core::dictation::key_event(vk, crate::keymap::from_vk(vk), is_down)
        {
            return LRESULT(1);
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Spawns a dedicated OS thread with a message loop for the keyboard hook.
pub fn install() -> Result<(), String> {
    let (ready, registered) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("hotkey-hook".into())
        .spawn(move || unsafe {
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
                Ok(_) => { let _ = ready.send(Ok(())); }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
        })
        .map_err(|e| e.to_string())?;
    registered.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}
