use dictto_core::platform::{Sound, Sounds};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::core::PCWSTR;

static START_WAV: &[u8] = include_bytes!("../assets/sounds/start.wav");
static STOP_WAV: &[u8] = include_bytes!("../assets/sounds/stop.wav");

/// Plays sound effects without blocking.
///
/// Uses Win32 PlaySound with SND_MEMORY|SND_ASYNC for native OS audio pipeline —
/// identical quality to the system media player, zero resampling.
/// Static include_bytes! data never gets freed, so SND_MEMORY|SND_ASYNC is safe.
pub struct WindowsSounds;

impl Sounds for WindowsSounds {
    fn play(&self, sound: Sound) {
        let bytes: &[u8] = match sound {
            Sound::Start => START_WAV,
            Sound::Stop => STOP_WAV,
        };

        let _ = unsafe {
            let ptr = PCWSTR(bytes.as_ptr() as *const u16);
            PlaySoundW(ptr, None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT)
        };
    }
}
