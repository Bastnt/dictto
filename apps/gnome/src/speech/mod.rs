//! Speech on GNOME: PipeWire playback, the `dictto_microphone` route and AT-SPI selection.
pub mod defaults;
pub mod selection;
pub mod virtual_mic;

use crate::audio::{Pipewire, PipewirePlayer};
use crate::dbus::shell::ShellProxy;
use dictto_core::speech::cache::CacheFiles;
use dictto_core::speech::SpeechPlatform;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use selection::AtspiSelection;

/// One journal per login session, like Windows' per-session file, so two sessions of the
/// same user never restore each other's microphone.
pub fn journal_path(data_dir: &Path, session: Result<String, String>) -> Result<PathBuf, String> {
    let id = session.map_err(|e| format!("Cannot identify the login session: {e}"))?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(format!("Unexpected login session id {id:?}."));
    }
    Ok(data_dir.join(format!("microphone-route-{id}.json")))
}

pub fn platform(pipewire: Arc<Pipewire>, shell: ShellProxy<'static>, journal: Result<PathBuf, String>) -> SpeechPlatform {
    let (virtual_mic, mic_route) = virtual_mic::pipewire_route(journal, pipewire.clone());
    SpeechPlatform {
        player: Arc::new(PipewirePlayer::new(pipewire)),
        virtual_mic,
        mic_route,
        selection: Arc::new(AtspiSelection::new(shell)),
        cache_files: CacheFiles::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_is_per_session_and_rejects_odd_ids() {
        let dir = Path::new("/data");
        assert_eq!(journal_path(dir, Ok("c2".into())).unwrap(), dir.join("microphone-route-c2.json"));
        assert!(journal_path(dir, Ok("../x".into())).is_err());
        assert!(journal_path(dir, Ok(String::new())).is_err());
        assert!(journal_path(dir, Err("no logind".into())).unwrap_err().contains("no logind"));
    }
}
