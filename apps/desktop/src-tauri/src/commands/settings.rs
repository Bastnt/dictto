use dictto_core::Runtime;
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub fn get_setting(key: String, runtime: State<'_, Runtime>) -> Result<Option<String>, String> {
    Ok(runtime.get_settings()?.remove(&key))
}

#[tauri::command]
pub fn set_setting(key: String, value: String, runtime: State<'_, Runtime>) -> Result<(), String> {
    runtime.set_setting(&key, &value)
}

#[tauri::command]
pub fn get_all_settings(runtime: State<'_, Runtime>) -> Result<HashMap<String, String>, String> {
    runtime.get_settings()
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
