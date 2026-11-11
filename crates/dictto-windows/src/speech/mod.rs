//! Windows speech output: WinMM playback, VB-CABLE routing and UI Automation selection.
mod communications;
mod playback;
mod selection;
mod virtual_mic;

use async_trait::async_trait;
use dictto_core::speech::{Player, SpeechPlatform};
use std::fs::File;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

pub use selection::WindowsSelection;
pub use virtual_mic::Manager;

struct WindowsPlayer {
    microphone: Option<virtual_mic::Player>,
}

#[async_trait]
impl Player for WindowsPlayer {
    fn requires_complete_audio(&self) -> bool {
        self.microphone.is_some()
    }

    async fn chunk(&self, audio: Vec<u8>) -> Result<(), String> {
        playback::play(audio).await
    }
    async fn file(&self, path: &Path) -> Result<(), String> {
        if let Some(mic) = &self.microphone {
            let result = mic
                .play(hound::WavReader::open(path).map_err(|e| e.to_string())?)
                .await;
            return match (result, mic.finish()) {
                (Err(error), Err(restore)) => {
                    Err(format!("{error} Microphone restoration pending: {restore}"))
                }
                (result, restore) => result.and(restore),
            };
        }
        playback::play_file(path).await
    }
}

struct VirtualMic(Arc<Manager>);

impl dictto_core::speech::VirtualMic for VirtualMic {
    fn check_available(&self) -> Result<(), String> {
        self.0.check_available()
    }
    fn player(&self) -> Box<dyn Player + Send> {
        Box::new(WindowsPlayer {
            microphone: Some(virtual_mic::Player::new(self.0.clone())),
        })
    }
}

fn create_hidden_marker(path: &Path) -> std::io::Result<File> {
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    File::options()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(path)
}

/// `mic_route` stays with the app, which drives its session and shutdown lifecycle.
pub fn platform(mic_route: Arc<Manager>) -> SpeechPlatform {
    SpeechPlatform {
        player: Arc::new(WindowsPlayer { microphone: None }),
        virtual_mic: Arc::new(VirtualMic(mic_route)),
        selection: Arc::new(WindowsSelection),
        create_marker: create_hidden_marker,
    }
}
