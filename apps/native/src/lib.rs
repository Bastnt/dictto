pub mod host;
pub mod speech_api;
pub(crate) mod speech;
#[path = "../../desktop/src-tauri/src/services/mod.rs"]
pub mod services;
#[path = "../../desktop/src-tauri/src/instance.rs"]
pub mod instance;
pub mod runtime;
pub use runtime::NativeRuntime;
pub use services::audio::MicrophoneInfo;

use std::sync::{Arc, Mutex};
pub struct AppState {
    pub(crate) mic_route: Arc<speech::virtual_mic::Manager>,
    pub(crate) speech_cache: Mutex<Option<speech::cache::Cache>>,
    pub(crate) speech_audio_dir: std::path::PathBuf,
    pub sample_path: std::path::PathBuf,
    pub audio_recorder: Mutex<services::audio::recorder::AudioRecorder>,
    pub db: Mutex<services::db::Database>,
    pub http_client: reqwest::Client,
    pub device_monitor: Arc<services::audio::device_monitor::DeviceMonitorState>,
    pub device_monitor_shutdown: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}
