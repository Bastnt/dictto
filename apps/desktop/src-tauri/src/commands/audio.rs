pub use dictto_windows::MicrophoneInfo;

#[tauri::command]
pub fn list_microphones() -> Result<Vec<MicrophoneInfo>, String> {
    dictto_windows::recorder::list_microphones().map_err(|e| e.to_string())
}

/// Returns the friendly name of the device the monitor currently tracks.
/// Used by the frontend to display "Auto-detect (Device Name)".
#[tauri::command]
pub fn get_current_microphone(state: tauri::State<'_, std::sync::Arc<dictto_core::CoreState>>) -> String {
    state.device_monitor.get_device_name()
}

/// Start continuous recording from UI click (bar pill click)
#[tauri::command]
pub fn bar_start_recording() {
    dictto_core::dictation::bar_start();
}

/// Stop continuous recording and process the audio (bar Stop button)
#[tauri::command]
pub fn bar_stop_recording() {
    dictto_core::dictation::bar_stop();
}

/// Cancel continuous recording and discard audio (bar Cancel button)
#[tauri::command]
pub fn bar_cancel_recording() {
    dictto_core::dictation::bar_cancel();
}

#[tauri::command]
pub fn set_hotkey_capture(active: bool) {
    dictto_core::dictation::set_capture(active);
}
