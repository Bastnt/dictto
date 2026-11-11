use crate::audio::{AudioError, DeviceMonitorState, DeviceSelection, MicrophoneInfo};
use std::sync::Arc;
use crate::host::AppHandle;
use std::sync::OnceLock;

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
    pub wording: &'static Wording,
}

/// User-facing messages that name OS facilities. They are read by free functions
/// (friendly errors, the keystore) that hosts also call outside the runtime.
pub struct Wording {
    /// Shown when no capture device exists.
    pub no_microphone: &'static str,
    /// Shown when the capture device cannot be opened.
    pub microphone_unavailable: &'static str,
    /// Where API keys are stored, as in "API key was not persisted in {credential_store}".
    pub credential_store: &'static str,
}

pub const GENERIC_WORDING: Wording = Wording {
    no_microphone: "No microphone is available. Connect one, then try again.",
    microphone_unavailable:
        "Cannot open the microphone. Check the input device in Settings > Audio and the system microphone permissions.",
    credential_store: "the system keyring",
};

static WORDING: OnceLock<&'static Wording> = OnceLock::new();

/// The first call wins; `Runtime` calls it with `Platform::wording`, and a host that reads
/// the messages before building its runtime calls it first.
pub fn set_wording(wording: &'static Wording) {
    let _ = WORDING.set(wording);
}

pub fn wording() -> &'static Wording {
    WORDING.get().copied().unwrap_or(&GENERIC_WORDING)
}
