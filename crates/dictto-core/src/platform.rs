use crate::audio::{AudioError, DeviceSelection};
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
