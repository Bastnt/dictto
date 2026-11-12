//! Speech on GNOME: PipeWire playback, the `dictto_microphone` route and AT-SPI selection.
pub mod defaults;
pub mod selection;
pub mod virtual_mic;

use crate::audio::{Pipewire, PipewirePlayer};
use crate::dbus::shell::ShellProxy;
use dictto_core::speech::cache::CacheFiles;
use dictto_core::speech::SpeechPlatform;
use std::path::Path;
use std::sync::Arc;

pub use selection::AtspiSelection;

/// The microphone route journals under `data_dir`.
pub fn platform(pipewire: Arc<Pipewire>, shell: ShellProxy<'static>, data_dir: &Path) -> SpeechPlatform {
    let journal = data_dir.join("microphone-route.json");
    let (virtual_mic, mic_route) = virtual_mic::pipewire_route(journal, pipewire.clone());
    SpeechPlatform {
        player: Arc::new(PipewirePlayer::new(pipewire)),
        virtual_mic,
        mic_route,
        selection: Arc::new(AtspiSelection::new(shell)),
        cache_files: CacheFiles::default(),
    }
}
