//! Windows speech output: WinMM playback, VB-CABLE routing and UI Automation selection.
mod communications;
mod playback;
mod selection;
mod virtual_mic;

use async_trait::async_trait;
use dictto_core::speech::cache::CacheFiles;
use dictto_core::speech::{Player, SpeechPlatform};
use std::fs::File;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

pub use communications::WindowsDefaults;
pub use selection::WindowsSelection;
pub use virtual_mic::{manager, CableOutput, Manager};

struct Speakers;

#[async_trait]
impl Player for Speakers {
    async fn chunk(&self, audio: Vec<u8>) -> Result<(), String> {
        playback::play(audio).await
    }
    async fn file(&self, path: &Path) -> Result<(), String> {
        playback::play_file(path).await
    }
}

fn create_hidden_marker(path: &Path) -> io::Result<File> {
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    File::options()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(path)
}

// 32 and 33 are ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION: another process holds the file.
fn locked(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33)) || error.kind() == io::ErrorKind::PermissionDenied
}

const CACHE_FILES: CacheFiles = CacheFiles {
    create_marker: create_hidden_marker,
    locked,
};

/// `mic_route` stays with the app, which drives its session and shutdown lifecycle.
pub fn platform(mic_route: &Arc<Manager>) -> SpeechPlatform {
    SpeechPlatform {
        player: Arc::new(Speakers),
        virtual_mic: dictto_core::speech::virtual_mic::virtual_mic(mic_route),
        selection: Arc::new(WindowsSelection),
        cache_files: CACHE_FILES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictto_core::speech::cache::Cache;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SERIAL: AtomicU64 = AtomicU64::new(1);
    fn key(i: u64) -> String {
        format!("{i:064x}")
    }
    fn render(dir: &Path, sample: i16) -> PathBuf {
        let path = dir.join(format!(
            "dictto-tts-render-{}-{}.partial",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut wav = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        wav.write_sample(sample).unwrap();
        wav.finalize().unwrap();
        path
    }

    #[test]
    fn unchanged_cache_hit_needs_no_audio_read_and_locked_eviction_keeps_working() {
        let dir = std::env::temp_dir().join(format!("dictto-locked-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cache = Cache::new(dir.clone(), CACHE_FILES).unwrap();
        let path = cache.put_file(&key(1), &render(&dir, 1)).unwrap();
        cache.put_file(&key(2), &render(&dir, 2)).unwrap();
        let mut cache = Cache::new(dir.clone(), CACHE_FILES).unwrap();
        let lock = File::options()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(File::open(&path).is_err());
        assert_eq!(cache.get_path(&key(1)).unwrap().unwrap(), path);
        for i in 3..=8 {
            cache.put_file(&key(i), &render(&dir, i as i16)).unwrap();
        }
        assert!(cache.get_path(&key(1)).unwrap().is_none());
        assert!(path.exists());
        drop(lock);
        cache.put_file(&key(9), &render(&dir, 9)).unwrap();
        assert!(!path.exists());
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
