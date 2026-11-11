use super::communications::{cable_endpoint, WindowsDefaults};
use super::playback;
use dictto_core::speech::virtual_mic::{MicOutput, MicRoute};
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use windows::Win32::Media::Audio::eRender;

pub type Manager = MicRoute<WindowsDefaults, CableOutput>;

/// The journal is per Windows session so fast user switching never crosses restores.
pub fn manager(data_dir: &Path) -> Manager {
    let mut session = 0;
    let journal = unsafe {
        windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(
            std::process::id(),
            &mut session,
        )
    }
    .map(|()| data_dir.join(format!("communications-route-{session}.json")))
    .map_err(|e| e.to_string());
    MicRoute::new(journal, WindowsDefaults, CableOutput)
}

/// VB-CABLE's playback endpoint, which feeds its capture endpoint.
pub struct CableOutput;

#[async_trait::async_trait]
impl MicOutput for CableOutput {
    type Target = u32;
    fn ensure_present(&self) -> Result<(), String> {
        cable_endpoint(eRender).map(drop)
    }
    fn locate(&self) -> Result<u32, String> {
        playback::device_id_for_endpoint(&cable_endpoint(eRender)?)
    }
    async fn wait_ready(&self) {
        // Give communications clients time to reopen capture after the default changes.
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    async fn play<R, C>(&self, wav: hound::WavReader<R>, target: &u32, check: C) -> Result<(), String>
    where
        R: Read + Send,
        C: FnMut() -> Result<(), String> + Send,
    {
        playback::play_to(wav, *target, check).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictto_core::speech::route::Defaults;
    use dictto_core::speech::virtual_mic::RoutedPlayer;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore = "requires an installed VB-CABLE and temporarily changes the communications mic"]
    async fn cable_playback_restores_the_communications_input() {
        let folder = std::env::temp_dir().join(format!("dictto-cable-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let manager = Arc::new(manager(&folder));
        manager.set_enabled(true).unwrap();
        let original = WindowsDefaults.current().unwrap();
        let target = WindowsDefaults.target().unwrap();
        eprintln!("Original communications input is cable: {}", original == target);
        let mut wav = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 24000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for frame in 0..24000 {
            let signal = (f64::from(frame) * std::f64::consts::TAU * 631.0 / 24000.0).sin();
            writer.write_sample((signal * 3000.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
        wav.set_position(0);
        let player = RoutedPlayer::new(manager);
        let result = player.play(hound::WavReader::new(wav).unwrap()).await;
        let during = WindowsDefaults.current();
        player.finish().unwrap();
        assert_eq!(WindowsDefaults.current().unwrap(), original);
        result.unwrap();
        assert_eq!(during.unwrap(), target);
        std::fs::remove_dir(folder).unwrap();
    }
}
