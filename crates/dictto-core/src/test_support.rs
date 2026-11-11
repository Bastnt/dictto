//! Inert platform fakes for tests that need a `CoreState` but never touch devices.

pub(crate) struct NoRecorder;
impl crate::platform::Recorder for NoRecorder {
    fn set_app_handle(&mut self, _: crate::host::AppHandle) {}
    fn start(&mut self, _: crate::audio::DeviceSelection) -> Result<(), crate::audio::AudioError> { unreachable!() }
    fn stop(&mut self) -> Result<Vec<u8>, crate::audio::AudioError> { unreachable!() }
    fn cancel(&mut self) {}
    fn is_recording(&self) -> bool { false }
}
pub(crate) struct NoSounds;
impl crate::platform::Sounds for NoSounds {
    fn play(&self, _: crate::platform::Sound) {}
}
pub(crate) struct NoDevices;
impl crate::platform::Devices for NoDevices {
    fn list_microphones(&self) -> Result<Vec<crate::audio::MicrophoneInfo>, crate::audio::AudioError> { Ok(vec![]) }
}
pub(crate) struct NoInjector;
#[async_trait::async_trait]
impl crate::platform::TextInjector for NoInjector {
    async fn inject(&self, _: &str, _: u64) -> Result<std::time::Instant, crate::platform::InjectionError> {
        unreachable!()
    }
}
pub(crate) fn core_state(db: crate::db::Database) -> crate::CoreState {
    crate::CoreState {
        platform: crate::platform::Platform {
            injector: std::sync::Arc::new(NoInjector),
            sounds: std::sync::Arc::new(NoSounds),
            devices: std::sync::Arc::new(NoDevices),
        },
        audio_recorder: std::sync::Mutex::new(Box::new(NoRecorder)),
        db: std::sync::Mutex::new(db),
        http_client: reqwest::Client::new(),
        device_monitor: std::sync::Arc::new(crate::audio::DeviceMonitorState::new()),
        device_monitor_shutdown: std::sync::Mutex::new(None),
    }
}
