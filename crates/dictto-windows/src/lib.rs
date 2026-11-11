//! Windows platform services shared by the Dictto apps.
pub mod autostart;
pub mod device_monitor;
pub mod injector;
pub mod instance;
pub mod keyboard;
pub mod keymap;
pub mod recorder;
pub mod sound;

pub use dictto_core::audio::MicrophoneInfo;
pub use injector::WindowsInjector;
pub use recorder::{WindowsDevices, WindowsRecorder};
pub use sound::WindowsSounds;

/// The Windows implementations of the traits the core reaches the OS through.
pub fn platform() -> dictto_core::platform::Platform {
    dictto_core::platform::Platform {
        injector: std::sync::Arc::new(WindowsInjector),
        sounds: std::sync::Arc::new(WindowsSounds),
        devices: std::sync::Arc::new(WindowsDevices),
    }
}
