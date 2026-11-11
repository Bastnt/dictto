//! Platform-neutral core shared by the Dictto apps.
pub mod audio;
pub mod dictation;
pub mod events;
pub mod keys;
pub mod db;
pub mod host;
pub mod llm;
pub mod pipeline;
pub mod platform;
mod runtime;
#[cfg(feature = "speech")]
pub mod speech;
pub mod transcription;
pub mod warmup;

pub use runtime::Runtime;

#[cfg(test)]
mod endpoint_tests;
#[cfg(test)]
mod test_support;

use std::sync::{Arc, Mutex};

pub struct CoreState {
    pub platform: platform::Platform,
    pub audio_recorder: Mutex<Box<dyn platform::Recorder>>,
    pub db: Mutex<db::Database>,
    pub http_client: reqwest::Client,
    pub device_monitor: Arc<audio::DeviceMonitorState>,
    pub device_monitor_shutdown: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}
