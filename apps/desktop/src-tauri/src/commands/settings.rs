use crate::services::autostart;
use dictto_core::CoreState;
use std::sync::Arc;
use std::collections::HashMap;
use tauri::{Emitter, State};

#[tauri::command]
pub fn get_setting(key: String, state: State<'_, Arc<CoreState>>) -> Result<Option<String>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    Ok(db.get_setting(&key))
}

#[tauri::command]
pub fn set_setting(
    key: String,
    value: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<CoreState>>,
) -> Result<(), String> {
    let hotkey = if key == "hotkey" {
        Some(dictto_core::keys::Chord::parse(&value)?)
    } else { None };
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.set_setting(&key, &value).map_err(|e| e.to_string())?;
    } // db lock released here
    if let Some(chord) = hotkey { dictto_core::dictation::configure(chord); }

    // Handle auto-start toggle
    if key == "auto_start" {
        if let Ok(exe_path) = std::env::current_exe() {
            let exe_str = exe_path.to_string_lossy().to_string();
            if value == "true" {
                let _ = autostart::enable_autostart(&exe_str);
            } else {
                let _ = autostart::disable_autostart();
            }
        }
    }

    // Microphone device change is handled dynamically — the new device
    // is read from settings on each start() call, no pre-warming needed.

    // Notify all windows of the setting change
    let _ = app.emit(
        "setting-changed",
        serde_json::json!({ "key": key, "value": value }),
    );

    Ok(())
}

#[tauri::command]
pub fn get_all_settings(state: State<'_, Arc<CoreState>>) -> Result<HashMap<String, String>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_all_settings().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_api_key(api_key: String) -> Result<(), String> {
    dictto_core::db::keystore::set_api_key(&api_key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn has_api_key() -> Result<bool, String> {
    Ok(dictto_core::db::keystore::get_api_key().is_ok())
}

#[tauri::command]
pub fn remove_api_key() -> Result<(), String> {
    dictto_core::db::keystore::delete_api_key().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_api_key_hint() -> Result<String, String> {
    let key = dictto_core::db::keystore::get_api_key().map_err(|e| e.to_string())?;
    if key.len() > 3 {
        let last3 = &key[key.len() - 3..];
        Ok(format!("••••••••{}", last3))
    } else {
        Ok("••••••••".to_string())
    }
}
