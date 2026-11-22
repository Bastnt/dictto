//! The application runtime hosts embed: shared state, settings, dictation lifecycle and
//! the events that travel to the host's UI through the sink.
use crate::events;
use crate::audio::MicrophoneInfo;
use crate::host::{self, AppHandle, Emitter};
use crate::platform::Platform;
use crate::transcription::catalog::ModelCatalog;
#[cfg(feature = "speech")]
use crate::speech::{MicRouteControl, SpeechPlatform, SpeechState};
use crate::CoreState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Runtime {
    inner: Arc<RuntimeState>,
}
struct RuntimeState {
    core: Arc<CoreState>,
    // Registered on the host next to the core state; absent for hosts without speech.
    #[cfg(feature = "speech")]
    speech: Option<Arc<SpeechState>>,
    app: AppHandle,
    started: AtomicBool,
    // The OS input hook cannot be removed, so a retried start must not install it twice.
    input_installed: AtomicBool,
    // Kept until a refresh or a new endpoint or key. The epoch lets a reply that started
    // before the endpoint changed be dropped instead of cached.
    models: Mutex<(u64, Option<ModelCatalog>)>,
}
impl Runtime {
    pub fn new(
        data_dir: PathBuf,
        platform: Platform,
        sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Self::build(
            data_dir,
            platform,
            #[cfg(feature = "speech")]
            None,
            sink,
        )
    }
    /// A runtime that also reads selections aloud. Kept apart from `new` so a host without
    /// speech compiles unchanged when another crate in the build enables the feature.
    #[cfg(feature = "speech")]
    pub fn with_speech(
        data_dir: PathBuf,
        platform: Platform,
        speech: SpeechPlatform,
        sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Self::build(data_dir, platform, Some(speech), sink)
    }
    fn build(
        data_dir: PathBuf,
        platform: Platform,
        #[cfg(feature = "speech")] speech: Option<SpeechPlatform>,
        sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        host::async_runtime::initialize()?;
        crate::platform::set_wording(platform.wording);
        let recorder = (platform.new_recorder)();
        #[cfg(feature = "speech")]
        let speech_dir = data_dir.clone();
        let core = Arc::new(CoreState {
            platform,
            audio_recorder: Mutex::new(recorder),
            db: Mutex::new(crate::db::Database::new(data_dir).map_err(|e| e.to_string())?),
            http_client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?,
            device_monitor: Arc::new(crate::audio::DeviceMonitorState::new()),
            device_monitor_shutdown: Mutex::new(None),
        });
        #[cfg(feature = "speech")]
        let speech = speech
            .map(|platform| SpeechState::new(core.clone(), &speech_dir, platform).map(Arc::new))
            .transpose()?;
        let app = AppHandle::new(&core, Arc::new(sink));
        #[cfg(feature = "speech")]
        let app = match &speech {
            Some(speech) => app.with_state(speech),
            None => app,
        };
        core.audio_recorder
            .lock()
            .map_err(|e| e.to_string())?
            .set_app_handle(app.clone());
        Ok(Self {
            inner: Arc::new(RuntimeState {
                core,
                #[cfg(feature = "speech")]
                speech,
                app,
                started: AtomicBool::new(false),
                input_installed: AtomicBool::new(false),
                models: Mutex::new((0, None)),
            }),
        })
    }
    #[cfg(feature = "speech")]
    fn mic_route(&self) -> Option<&Arc<dyn MicRouteControl>> {
        self.inner.speech.as_ref().map(|s| &s.speech.mic_route)
    }
    pub fn start(&self) -> Result<(), String> {
        if self.inner.started.load(Ordering::Acquire) {
            return Ok(());
        }
        #[cfg(feature = "speech")]
        if let Some(Err(error)) = self.mic_route().map(|route| route.recover()) {
            self.report_route_error(error);
        }
        let settings = self.get_settings()?;
        #[cfg(feature = "speech")]
        if let Some(route) = self.mic_route() {
            route.set_enabled(settings.get("tts_virtual_mic").is_some_and(|v| v == "true"))?;
        }
        if settings.get("auto_start").map(String::as_str) == Some("true") {
            // Refreshes entries written by older versions that lack the launch flag.
            if let Err(error) = self.inner.core.platform.services.set_autostart(true) {
                log::warn!("Could not refresh the auto-start entry: {error}");
            }
        }
        if self.inner.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Err(error) = self.start_dictation(&settings) {
            self.inner.started.store(false, Ordering::Release);
            return Err(error);
        }
        let tx = self
            .inner
            .core
            .platform
            .services
            .spawn_device_monitor(self.inner.core.device_monitor.clone(), self.inner.app.clone());
        *self
            .inner
            .core
            .device_monitor_shutdown
            .lock()
            .map_err(|e| e.to_string())? = Some(tx);
        crate::warmup::schedule(&self.inner.app);
        Ok(())
    }
    fn start_dictation(&self, settings: &HashMap<String, String>) -> Result<(), String> {
        #[cfg(feature = "speech")]
        if self.inner.speech.is_some() {
            crate::speech::configure(crate::speech::parse_bindings(settings, false)?);
        }
        #[cfg(not(feature = "speech"))]
        let _ = settings;
        // Installed first: a failure leaves the dictation channels unset so a retry can succeed.
        if !self.inner.input_installed.load(Ordering::Acquire) {
            self.inner.core.platform.services.install_input()?;
            self.inner.input_installed.store(true, Ordering::Release);
        }
        crate::dictation::start(&self.inner.app)
    }
    #[cfg(feature = "speech")]
    pub fn speech_audio_dir(&self) -> Option<PathBuf> {
        self.inner.speech.as_ref().map(|s| s.audio_dir.clone())
    }
    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.inner.core.db.lock().ok()?.get_setting(key)
    }
    pub fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        self.inner
            .core
            .db
            .lock()
            .map_err(|e| e.to_string())?
            .get_all_settings()
            .map_err(|e| e.to_string())
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let chord = if key == "hotkey" {
            Some(crate::keys::Chord::parse(value)?)
        } else {
            None
        };
        if key == "auto_start" {
            self.inner
                .core
                .platform
                .services
                .set_autostart(value == "true")?;
        }
        {
            let db = self.inner.core.db.lock().map_err(|e| e.to_string())?;
            #[cfg(feature = "speech")]
            let bindings = if self.inner.speech.is_some()
                && (key == "hotkey"
                    || crate::speech::BINDINGS.iter().any(|(name, _)| *name == key))
            {
                let mut settings = db.get_all_settings().map_err(|e| e.to_string())?;
                settings.insert(key.into(), value.into());
                Some(crate::speech::parse_bindings(&settings, true)?)
            } else {
                None
            };
            db.set_setting(key, value).map_err(|e| e.to_string())?;
            if key == "api_base_url" {
                self.forget_models();
            }
            #[cfg(feature = "speech")]
            if let Some(bindings) = bindings {
                crate::speech::configure(bindings);
            }
            if let Some(chord) = chord {
                crate::dictation::configure(chord);
            }
        }
        #[cfg(feature = "speech")]
        if key == "tts_virtual_mic" {
            if let Some(Err(error)) = self.mic_route().map(|route| route.set_enabled(value == "true")) {
                self.report_route_error(error);
            }
        }
        self.inner
            .app
            .emit(
                events::SETTING_CHANGED,
                serde_json::json!({"key":key, "value":value}),
            )
            .map_err(|e| e.to_string())
    }
    /// Models the configured endpoint advertises, from the cache unless `refresh`.
    pub async fn models(&self, refresh: bool) -> Result<ModelCatalog, String> {
        let epoch = {
            let cache = self.inner.models.lock().map_err(|e| e.to_string())?;
            match &cache.1 {
                Some(models) if !refresh => return Ok(models.clone()),
                _ => cache.0,
            }
        };
        let base = self.get_settings()?.remove("api_base_url").ok_or("Set an API base URL first")?;
        let key = crate::db::keystore::get_api_key()?;
        let models = crate::transcription::catalog::discover(&self.inner.core.http_client, &base, &key).await?;
        let mut cache = self.inner.models.lock().map_err(|e| e.to_string())?;
        if cache.0 == epoch {
            cache.1 = Some(models.clone());
        }
        Ok(models)
    }
    fn forget_models(&self) {
        if let Ok(mut cache) = self.inner.models.lock() {
            *cache = (cache.0 + 1, None);
        }
    }
    /// `models` for hosts without an async caller; answers with `MODEL_CATALOG` so a stale
    /// reply can be told apart by its generation.
    pub fn discover_models(&self, generation: u64, refresh: bool) {
        let runtime = self.clone();
        host::async_runtime::spawn(async move {
            let payload = match runtime.models(refresh).await {
                Ok(models) => serde_json::json!({
                    "generation": generation, "transcription": models.transcription, "chat": models.chat
                }),
                Err(error) => serde_json::json!({"generation": generation, "error": error}),
            };
            let _ = runtime.inner.app.emit(events::MODEL_CATALOG, payload);
        });
    }
    #[cfg(feature = "speech")]
    pub fn discover_speech_models(&self, generation: u64) {
        let runtime = self.clone();
        host::async_runtime::spawn(async move {
            let result = async {
                let settings = runtime.get_settings()?;
                let base = settings
                    .get("api_base_url")
                    .ok_or("Set an API base URL first")?;
                let key = crate::db::keystore::get_api_key()?;
                crate::speech::api::discover(&runtime.inner.core.http_client, base, &key).await
            }
            .await;
            let payload = match result {
                Ok(models) => serde_json::json!({"generation": generation, "models": models}),
                Err(error) => serde_json::json!({"generation": generation, "error": error}),
            };
            let _ = runtime.inner.app.emit(events::SPEECH_CATALOG, payload);
        });
    }
    pub fn has_key(&self) -> bool {
        crate::db::keystore::get_api_key().is_ok()
    }
    pub fn key_hint(&self) -> Result<String, String> {
        let key = crate::db::keystore::get_api_key()?;
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
        self.forget_models();
        crate::db::keystore::set_api_key(value)
    }
    pub fn remove_key(&self) -> Result<(), String> {
        self.forget_models();
        crate::db::keystore::delete_api_key()
    }
    pub fn microphones(&self) -> Result<Vec<MicrophoneInfo>, String> {
        self.inner.core.platform.devices.list_microphones().map_err(|e| e.to_string())
    }
    pub fn current_microphone(&self) -> String {
        self.inner.core.device_monitor.get_device_name()
    }
    pub fn set_hotkey_capture(&self, active: bool) {
        crate::dictation::set_capture(active);
    }
    /// Drops key events while the session is locked or inactive; independent of
    /// `set_hotkey_capture`.
    pub fn input_suspended(&self, suspended: bool) {
        crate::dictation::set_input_suspended(suspended);
    }
    /// Releases held keys, e.g. when the keyboard holding them disappears.
    pub fn reset_input(&self) {
        crate::dictation::reset_input();
    }
    pub fn start_recording(&self) {
        crate::dictation::bar_start();
    }
    pub fn stop_recording(&self) {
        crate::dictation::bar_stop();
    }
    pub fn cancel_recording(&self) {
        crate::dictation::bar_cancel();
    }
    #[cfg(feature = "speech")]
    fn report_route_error(&self, error: String) {
        let name = self
            .mic_route()
            .map_or_else(|| "default microphone".into(), |route| route.input_name());
        let mut chars = name.chars();
        let title: String = chars.next().into_iter().flat_map(char::to_uppercase).chain(chars).collect();
        log::error!("{title} recovery: {error}");
        let message = format!("Could not restore the {name}. {error}");
        let _ = self
            .inner
            .app
            .emit(events::SPEECH_ERROR, serde_json::json!({"message":message}));
    }
    pub fn session_changed(&self, suspended: bool) {
        #[cfg(feature = "speech")]
        if let Some(Err(error)) = self.mic_route().map(|route| route.session_changed(suspended)) {
            self.report_route_error(error);
        }
        #[cfg(not(feature = "speech"))]
        let _ = suspended;
    }
    pub fn shutdown(&self) {
        #[cfg(feature = "speech")]
        if let Some(route) = self.mic_route() {
            route.shutdown();
        }
        self.set_hotkey_capture(true);
        self.cancel_recording();
        if let Ok(mut recorder) = self.inner.core.audio_recorder.lock() {
            let mut replacement = (self.inner.core.platform.new_recorder)();
            replacement.set_app_handle(self.inner.app.clone());
            let retired = std::mem::replace(&mut *recorder, replacement);
            drop(recorder);
            drop(retired);
        }
        if let Ok(mut tx) = self.inner.core.device_monitor_shutdown.lock() {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
            }
        }
    }
}

impl Drop for RuntimeState {
    fn drop(&mut self) {
        if let Ok(mut tx) = self.core.device_monitor_shutdown.lock() {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(
        path: PathBuf,
        sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static,
    ) -> Runtime {
        #[cfg(feature = "speech")]
        let runtime = Runtime::with_speech(
            path,
            crate::test_support::platform(),
            crate::speech::tests::platform(),
            sink,
        );
        #[cfg(not(feature = "speech"))]
        let runtime = Runtime::new(path, crate::test_support::platform(), sink);
        runtime.unwrap()
    }

    #[cfg(feature = "speech")]
    #[test]
    fn runtime_without_speech_still_sets_the_hotkey_when_the_feature_is_compiled_in() {
        let path = std::env::temp_dir().join(format!("dictto-core-nospeech-{}", std::process::id()));
        {
            let runtime = Runtime::new(path.clone(), crate::test_support::platform(), |_, _| {}).unwrap();
            runtime.set_setting("hotkey", "Ctrl+Shift+F8").unwrap();
            assert_eq!(runtime.get_setting("hotkey").as_deref(), Some("Ctrl+Shift+F8"));
        }
        std::fs::remove_dir_all(path).unwrap();
    }

    #[cfg(feature = "speech")]
    #[test]
    fn route_errors_name_the_platform_input() {
        let path = std::env::temp_dir().join(format!("dictto-core-route-{}", std::process::id()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        {
            let runtime = runtime(path.clone(), move |name, payload| {
                sink.lock().unwrap().push((name.to_owned(), payload))
            });
            runtime.report_route_error("Busy.".into());
        }
        assert_eq!(
            events.lock().unwrap()[0],
            (
                crate::events::SPEECH_ERROR.into(),
                serde_json::json!({"message":"Could not restore the default microphone. Busy."})
            )
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn settings_use_shared_database_and_dispatch_events_without_starting_devices() {
        let path = std::env::temp_dir().join(format!(
            "dictto-core-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        {
            let runtime = runtime(path.clone(), move |name, payload| {
                sink.lock().unwrap().push((name.to_owned(), payload))
            });
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
                    events::SETTING_CHANGED.into(),
                    serde_json::json!({"key":"cleanup_enabled", "value":"false"})
                )
            );
        }
        {
            let db = crate::db::Database::new(path.clone()).unwrap();
            assert_eq!(db.get_setting("cleanup_enabled").as_deref(), Some("false"));
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}
