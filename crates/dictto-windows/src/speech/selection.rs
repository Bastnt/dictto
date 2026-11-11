//! Reads the focused selection through UI Automation without touching the clipboard.
use dictto_core::speech::{preferred_text, SelectionReader};

unsafe fn edit_selection(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::{
        Foundation::*,
        UI::{Controls::EM_GETSEL, WindowsAndMessaging::*},
    };
    let mut class = [0u16; 32];
    let len = GetClassNameW(hwnd, &mut class);
    if !String::from_utf16_lossy(&class[..len as usize]).eq_ignore_ascii_case("Edit")
        || GetWindowLongW(hwnd, GWL_STYLE) & ES_PASSWORD as i32 != 0
    {
        return None;
    }
    let send = |msg, w, l| {
        let mut result = 0;
        (SendMessageTimeoutW(
            hwnd,
            msg,
            WPARAM(w),
            LPARAM(l),
            SMTO_ABORTIFHUNG,
            250,
            Some(&mut result),
        )
        .0 != 0)
            .then_some(result)
    };
    let mut start = 0u32;
    let mut end = 0u32;
    send(
        EM_GETSEL,
        (&raw mut start) as usize,
        (&raw mut end) as isize,
    )?;
    if start >= end {
        return None;
    }
    let len = send(WM_GETTEXTLENGTH, 0, 0)?;
    if end as usize > len {
        return None;
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(len.checked_add(1)?).ok()?;
    buffer.resize(len + 1, 0u16);
    let copied = send(WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize)?;
    if (end as usize) > copied {
        return None;
    }
    Some(String::from_utf16_lossy(
        &buffer[start as usize..end as usize],
    ))
}

unsafe fn selection_from_element(
    automation: &windows::Win32::UI::Accessibility::IUIAutomation,
    mut element: windows::Win32::UI::Accessibility::IUIAutomationElement,
) -> windows::core::Result<String> {
    use windows::Win32::UI::Accessibility::*;
    let walker = automation.ControlViewWalker()?;
    loop {
        if element.CurrentIsPassword()?.as_bool() {
            break;
        }
        if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            if let Ok(ranges) = pattern.GetSelection() {
                let mut text = Vec::new();
                for i in 0..ranges.Length()? {
                    text.push(ranges.GetElement(i)?.GetText(-1)?.to_string());
                }
                let text = text.join("\n");
                if !text.trim().is_empty() {
                    return Ok(text);
                }
            }
        }
        if let Ok(hwnd) = element.CurrentNativeWindowHandle() {
            if let Some(text) = edit_selection(hwnd) {
                return Ok(text);
            }
        }
        let Ok(parent) = walker.GetParentElement(&element) else {
            break;
        };
        element = parent;
    }
    Ok(String::new())
}

pub struct WindowsSelection;

impl SelectionReader for WindowsSelection {
    fn read(&self) -> Result<String, String> {
        use windows::Win32::System::Com::*;
        use windows::Win32::UI::Accessibility::*;
        let selection = unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            let selection = (|| -> windows::core::Result<String> {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                selection_from_element(&automation, automation.GetFocusedElement()?)
            })()
            .ok();
            if init.is_ok() {
                CoUninitialize();
            }
            selection
        };
        preferred_text(selection, || {
            arboard::Clipboard::new()
                .and_then(|mut c| c.get_text())
                .map_err(|_| "Select text or copy it to the clipboard first.".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_edit_selection_is_read_without_copying() {
        use windows::core::w;
        use windows::Win32::{
            Foundation::*,
            System::Com::*,
            UI::{Accessibility::*, Controls::EM_SETSEL, WindowsAndMessaging::*},
        };
        let (ready, hwnd_rx) = std::sync::mpsc::channel();
        let window = std::thread::spawn(move || unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("EDIT"),
                w!("one two three"),
                WS_POPUP,
                0,
                0,
                180,
                40,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(4)), Some(LPARAM(7)));
            ready.send(hwnd.0 as usize).unwrap();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_APP {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = DestroyWindow(hwnd);
        });
        let hwnd = HWND(hwnd_rx.recv().unwrap() as _);
        let result = unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            let result = (|| -> windows::core::Result<String> {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                selection_from_element(&automation, automation.ElementFromHandle(hwnd)?)
            })();
            if init.is_ok() {
                CoUninitialize();
            }
            PostMessageW(Some(hwnd), WM_APP, WPARAM(0), LPARAM(0)).unwrap();
            result
        };
        window.join().unwrap();
        assert_eq!(result.unwrap(), "two");
    }
}
