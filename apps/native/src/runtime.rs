use crate::host::{AppHandle, Emitter};
use crate::{services, AppState, MicrophoneInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct NativeRuntime {
    inner: Arc<RuntimeState>,
}
struct RuntimeState {
    state: Arc<AppState>,
    app: AppHandle,
    started: AtomicBool,
}
impl NativeRuntime {
    pub fn new(
        data_dir: PathBuf,
        sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        crate::host::async_runtime::initialize()?;
        let state = Arc::new(AppState {
            audio_recorder: Mutex::new(services::audio::recorder::AudioRecorder::new()),
            db: Mutex::new(services::db::Database::new(data_dir).map_err(|e| e.to_string())?),
            http_client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?,
            device_monitor: Arc::new(services::audio::device_monitor::DeviceMonitorState::new()),
            device_monitor_shutdown: Mutex::new(None),
        });
        let app = AppHandle::new(&state, Arc::new(sink));
        state
            .audio_recorder
            .lock()
            .map_err(|e| e.to_string())?
            .set_app_handle(app.clone());
        Ok(Self {
            inner: Arc::new(RuntimeState {
                state,
                app,
                started: AtomicBool::new(false),
            }),
        })
    }
    pub fn start(&self) -> Result<(), String> {
        if self.inner.started.load(Ordering::Acquire) {
            return Ok(());
        }
        if self.get_settings()?.get("auto_start").map(String::as_str) == Some("true") {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            services::autostart::enable_autostart(&exe.to_string_lossy())?;
        }
        if self.inner.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Err(error) = services::hotkey::register_hotkey(&self.inner.app) {
            self.inner.started.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        #[cfg(windows)]
        {
            let tx = services::audio::device_monitor::spawn_monitor(
                self.inner.state.device_monitor.clone(),
                self.inner.app.clone(),
            );
            *self
                .inner
                .state
                .device_monitor_shutdown
                .lock()
                .map_err(|e| e.to_string())? = Some(tx);
        }
        services::warmup::schedule(&self.inner.app);
        Ok(())
    }
    pub fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        self.inner
            .state
            .db
            .lock()
            .map_err(|e| e.to_string())?
            .get_all_settings()
            .map_err(|e| e.to_string())
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let chord = if key == "hotkey" {
            Some(services::hotkey::chord::Chord::parse(value)?)
        } else {
            None
        };
        if key == "auto_start" {
            if value == "true" {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                services::autostart::enable_autostart(&exe.to_string_lossy())?;
            } else {
                services::autostart::disable_autostart()?;
            }
        }
        self.inner
            .state
            .db
            .lock()
            .map_err(|e| e.to_string())?
            .set_setting(key, value)
            .map_err(|e| e.to_string())?;
        if let Some(chord) = chord {
            services::hotkey::configure(chord);
        }
        self.inner
            .app
            .emit(
                "setting-changed",
                serde_json::json!({"key":key, "value":value}),
            )
            .map_err(|e| e.to_string())
    }
    pub fn has_key(&self) -> bool {
        services::db::keystore::get_api_key().is_ok()
    }
    pub fn key_hint(&self) -> Result<String, String> {
        let key = services::db::keystore::get_api_key()?;
        let suffix: String = key
            .chars()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(if key.chars().count() > 3 {
            format!("********{suffix}")
        } else {
            "********".into()
        })
    }
    pub fn save_key(&self, value: &str) -> Result<(), String> {
        services::db::keystore::set_api_key(value)
    }
    pub fn remove_key(&self) -> Result<(), String> {
        services::db::keystore::delete_api_key()
    }
    pub fn microphones(&self) -> Result<Vec<MicrophoneInfo>, String> {
        services::audio::recorder::list_microphones().map_err(|e| e.to_string())
    }
    pub fn current_microphone(&self) -> String {
        self.inner.state.device_monitor.get_device_name()
    }
    pub fn set_hotkey_capture(&self, active: bool) {
        services::hotkey::set_hotkey_capture(active);
    }
    pub fn start_recording(&self) {
        services::hotkey::send_bar_start();
    }
    pub fn stop_recording(&self) {
        services::hotkey::send_bar_stop();
    }
    pub fn cancel_recording(&self) {
        services::hotkey::send_bar_cancel();
    }
    pub fn shutdown(&self) {
        self.set_hotkey_capture(true);
        if let Ok(mut recorder) = self.inner.state.audio_recorder.lock() {
            let mut replacement = services::audio::recorder::AudioRecorder::new();
            replacement.set_app_handle(self.inner.app.clone());
            let retired = std::mem::replace(&mut *recorder, replacement);
            drop(recorder);
            drop(retired);
        }
        if let Ok(mut tx) = self.inner.state.device_monitor_shutdown.lock() {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_use_shared_database_and_dispatch_events_without_starting_devices() {
        let path = std::env::temp_dir().join(format!(
            "dictto-native-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        {
            let runtime = NativeRuntime::new(path.clone(), move |name, payload| {
                sink.lock().unwrap().push((name.to_owned(), payload))
            })
            .unwrap();
            let old_hotkey = runtime.get_settings().unwrap()["hotkey"].clone();
            assert!(runtime.set_setting("hotkey", "invalid").is_err());
            assert_eq!(runtime.get_settings().unwrap()["hotkey"], old_hotkey);
            runtime.set_setting("cleanup_enabled", "false").unwrap();
            let clone = runtime.clone();
            assert_eq!(clone.get_settings().unwrap()["cleanup_enabled"], "false");
            assert!(!runtime.inner.started.load(Ordering::Acquire));
            assert_eq!(
                events.lock().unwrap()[0],
                (
                    "setting-changed".into(),
                    serde_json::json!({"key":"cleanup_enabled", "value":"false"})
                )
            );
        }
        {
            let db = services::db::Database::new(path.clone()).unwrap();
            assert_eq!(db.get_setting("cleanup_enabled").as_deref(), Some("false"));
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}

impl Drop for RuntimeState {
    fn drop(&mut self) {
        if let Ok(mut tx) = self.state.device_monitor_shutdown.lock() {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
            }
        }
    }
}
