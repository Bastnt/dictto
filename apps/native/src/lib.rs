pub use dictto_core::host;
pub mod speech_api;
pub(crate) mod speech;
pub use dictto_windows::instance;
pub mod runtime;
pub use dictto_core::audio::MicrophoneInfo;
pub use runtime::NativeRuntime;

use dictto_core::CoreState;
use std::sync::{Arc, Mutex};

/// Native-only speech state, registered on the host next to `CoreState`.
/// Derefs to `CoreState` so speech code reads shared fields through one handle.
pub struct SpeechState {
    pub(crate) core: Arc<CoreState>,
    pub(crate) mic_route: Arc<speech::virtual_mic::Manager>,
    pub(crate) speech_cache: Mutex<Option<speech::cache::Cache>>,
    pub(crate) speech_audio_dir: std::path::PathBuf,
    pub sample_path: std::path::PathBuf,
}

impl std::ops::Deref for SpeechState {
    type Target = CoreState;
    fn deref(&self) -> &CoreState {
        &self.core
    }
}
