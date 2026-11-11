//! Platform-neutral core shared by the Dictto apps.
pub mod audio;
pub mod chord;
pub mod db;
pub mod host;
pub mod llm;
pub mod pipeline;
pub mod platform;
pub mod transcription;
pub mod warmup;

#[cfg(test)]
mod endpoint_tests;

use std::sync::{Arc, Mutex};

pub struct CoreState {
    pub audio_recorder: Mutex<Box<dyn platform::Recorder>>,
    pub db: Mutex<db::Database>,
    pub http_client: reqwest::Client,
    pub device_monitor: Arc<audio::DeviceMonitorState>,
    pub device_monitor_shutdown: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}
