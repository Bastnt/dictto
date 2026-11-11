use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

pub struct InstanceGuard(HANDLE);

impl InstanceGuard {
    pub fn acquire() -> Result<Self, String> {
        acquire()?.ok_or_else(|| "Dictto is already running. Quit it from the system tray before opening another build.".into())
    }
}

pub fn acquire() -> Result<Option<InstanceGuard>, String> {
    // Both frontends share hotkeys, clipboard injection and the same settings database.
    let handle = unsafe { CreateMutexW(None, false, w!("Local\\Dictto.Dictation.Session")) }
        .map_err(|error| format!("Unable to acquire Dictto session: {error}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { let _ = CloseHandle(handle); }
        return Ok(None);
    }
    Ok(Some(InstanceGuard(handle)))
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn second_variant_cannot_acquire_the_same_session() {
        let Some(first) = super::acquire().unwrap() else { return; };
        assert!(super::acquire().unwrap().is_none());
        drop(first);
        assert!(super::acquire().unwrap().is_some());
    }
}
