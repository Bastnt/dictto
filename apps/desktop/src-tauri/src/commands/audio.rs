use dictto_core::audio::MicrophoneInfo;
use dictto_core::Runtime;
use tauri::State;

#[tauri::command]
pub fn list_microphones(runtime: State<'_, Runtime>) -> Result<Vec<MicrophoneInfo>, String> {
    runtime.microphones()
}

/// Returns the friendly name of the device the monitor currently tracks.
/// Used by the frontend to display "Auto-detect (Device Name)".
#[tauri::command]
pub fn get_current_microphone(runtime: State<'_, Runtime>) -> String {
    runtime.current_microphone()
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
