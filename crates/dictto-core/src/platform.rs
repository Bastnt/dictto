use crate::audio::{AudioError, DeviceMonitorState, DeviceSelection, MicrophoneInfo};
use std::sync::Arc;
use crate::host::AppHandle;

pub trait Recorder: Send {
    fn set_app_handle(&mut self, app: AppHandle);
    fn start(&mut self, device: DeviceSelection) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<Vec<u8>, AudioError>;
    fn cancel(&mut self);
    fn is_recording(&self) -> bool;
}

#[derive(Debug, thiserror::Error)]
pub enum InjectionError {
    #[error("Clipboard error: {0}")]
    ClipboardError(String),
    #[error("Keyboard simulation error: {0}")]
    KeyboardError(String),
}

#[async_trait::async_trait]
pub trait TextInjector: Send + Sync {
    async fn inject(&self, text: &str, paste_delay_ms: u64) -> Result<std::time::Instant, InjectionError>;
}

#[derive(Debug, Clone, Copy)]
pub enum Sound {
    Start,
    Stop,
}

pub trait Sounds: Send + Sync {
    fn play(&self, sound: Sound);
}

pub trait Devices: Send + Sync {
    fn list_microphones(&self) -> Result<Vec<MicrophoneInfo>, AudioError>;
}

/// Session-level OS services the runtime starts and configures.
pub trait Services: Send + Sync {
    fn set_autostart(&self, enabled: bool) -> Result<(), String>;
    /// Starts feeding key events to `dictation::key_event`. Must fail without side
    /// effects so that a later retry can succeed.
    fn install_input(&self) -> Result<(), String>;
    /// Sending on, or dropping, the returned sender stops the monitor.
    fn spawn_device_monitor(
        &self,
        state: Arc<DeviceMonitorState>,
        app: AppHandle,
    ) -> std::sync::mpsc::Sender<()>;
}

/// OS services the core reaches through traits.
pub struct Platform {
    pub injector: Arc<dyn TextInjector>,
    pub sounds: Arc<dyn Sounds>,
    pub devices: Arc<dyn Devices>,
    pub services: Arc<dyn Services>,
    /// The runtime builds the recorder, and a fresh one at shutdown to release the old.
    pub new_recorder: Arc<dyn Fn() -> Box<dyn Recorder> + Send + Sync>,
}
