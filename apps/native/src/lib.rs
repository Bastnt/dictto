pub mod host;
#[path = "../../desktop/src-tauri/src/services/mod.rs"]
pub mod services;
#[path = "../../desktop/src-tauri/src/instance.rs"]
pub mod instance;
pub mod runtime;
pub use runtime::NativeRuntime;
pub use services::audio::MicrophoneInfo;

use std::sync::{Arc, Mutex};
pub struct AppState {
    pub audio_recorder: Mutex<services::audio::recorder::AudioRecorder>,
    pub db: Mutex<services::db::Database>,
    pub http_client: reqwest::Client,
    pub device_monitor: Arc<services::audio::device_monitor::DeviceMonitorState>,
    pub device_monitor_shutdown: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}
