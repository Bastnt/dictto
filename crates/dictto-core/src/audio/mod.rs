pub mod dsp;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(serde::Serialize, Clone, Debug)]
pub struct MicrophoneInfo {
    pub name: String,
    pub id: String,
    pub is_default: bool,
    pub form_factor: String,
}

/// Shared state between the device monitor thread and the rest of the app.
/// Fields are overwritten on each callback — never accumulated.
pub struct DeviceMonitorState {
    /// The endpoint ID of the last activated/default capture device.
    pub last_device_id: Mutex<Option<String>>,
    /// The friendly name of that device (for UI display).
    pub last_device_name: Mutex<String>,
    /// Flag set to true when any device change occurs. Frontend checks + clears.
    pub devices_changed: AtomicBool,
}

impl DeviceMonitorState {
    pub fn new() -> Self {
        Self {
            last_device_id: Mutex::new(None),
            last_device_name: Mutex::new(String::new()),
            devices_changed: AtomicBool::new(false),
        }
    }

    /// Read the current auto-detect device ID.
    pub fn get_device_id(&self) -> Option<String> {
        self.last_device_id.lock().unwrap().clone()
    }

    /// Read the current auto-detect device friendly name.
    pub fn get_device_name(&self) -> String {
        self.last_device_name.lock().unwrap().clone()
    }

    /// Check and clear the devices_changed flag.
    pub fn take_devices_changed(&self) -> bool {
        self.devices_changed.swap(false, Ordering::AcqRel)
    }
}

/// How the capture thread should resolve which device to use.
#[derive(Clone, Debug)]
pub enum DeviceSelection {
    PlaybackLoopback,
    /// Use the device tracked by the monitor (or fallback to system default).
    AutoDetect(Option<String>), // Option<endpoint_id>
    /// Use a specific device matched by friendly name.
    ByName(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("Already recording")]
    AlreadyRecording,
    #[error("Not recording")]
    NotRecording,
    #[error("No input device found")]
    NoDevice,
    #[error("Empty recording")]
    EmptyRecording,
    #[error("Audio device error: {0}")]
    DeviceError(String),
    #[error("Encoding error: {0}")]
    EncodingError(String),
}
