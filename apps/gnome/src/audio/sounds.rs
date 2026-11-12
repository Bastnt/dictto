//! Start and stop sounds. The core decides whether they play (the sound_effects setting).
use super::{Pcm, Pipewire};
use dictto_core::platform::{Sound, Sounds};
use std::sync::Arc;

// The same files the Windows app plays.
static START_WAV: &[u8] = include_bytes!("../../../../crates/dictto-core/assets/sounds/start.wav");
static STOP_WAV: &[u8] = include_bytes!("../../../../crates/dictto-core/assets/sounds/stop.wav");

pub struct PipewireSounds {
    pipewire: Arc<Pipewire>,
    start: Pcm,
    stop: Pcm,
}

impl PipewireSounds {
    pub fn new(pipewire: Arc<Pipewire>) -> Self {
        Self {
            pipewire,
            start: decode(START_WAV),
            stop: decode(STOP_WAV),
        }
    }
}

fn decode(wav: &[u8]) -> Pcm {
    hound::WavReader::new(wav)
        .map_err(|e| e.to_string())
        .and_then(Pcm::from_wav)
        .expect("embedded sound is a 16-bit PCM WAV")
}

impl Sounds for PipewireSounds {
    fn play(&self, sound: Sound) {
        let pcm = match sound {
            Sound::Start => &self.start,
            Sound::Stop => &self.stop,
        };
        self.pipewire.play_detached(pcm.clone(), "Notification");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_sounds_decode() {
        decode(START_WAV);
        decode(STOP_WAV);
    }
}
