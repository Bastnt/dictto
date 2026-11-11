//! Windows platform services shared by the Dictto apps.
pub mod autostart;
pub mod device_monitor;
pub mod injector;
pub mod instance;
pub mod keyboard;
pub mod keymap;
pub mod recorder;
pub mod services;
pub mod sound;
#[cfg(feature = "speech")]
pub mod speech;

pub use dictto_core::audio::MicrophoneInfo;
pub use injector::WindowsInjector;
pub use recorder::{WindowsDevices, WindowsRecorder};
pub use services::WindowsServices;
pub use sound::WindowsSounds;

/// The Windows implementations of the traits the core reaches the OS through.
pub fn platform() -> dictto_core::platform::Platform {
    dictto_core::platform::Platform {
        injector: std::sync::Arc::new(WindowsInjector),
        sounds: std::sync::Arc::new(WindowsSounds),
        devices: std::sync::Arc::new(WindowsDevices),
        services: std::sync::Arc::new(WindowsServices),
        new_recorder: std::sync::Arc::new(|| Box::new(WindowsRecorder::new())),
        wording: &WORDING,
    }
}

pub const WORDING: dictto_core::platform::Wording = dictto_core::platform::Wording {
    no_microphone: "Windows has no microphone available. Connect one or enable microphone recording in Remote Desktop, then reconnect.",
    microphone_unavailable: "Cannot open the microphone. Check the input device in Settings > Audio and Windows microphone access.",
    credential_store: "Windows Credential Manager",
};
