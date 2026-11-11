use super::communications::{self, WindowsDefaults};
use dictto_core::speech::route::{self, Defaults};
use super::playback;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::eRender;

struct Active {
    token: u64,
    target: String,
    checked: Instant,
}
#[derive(Default)]
struct State {
    generation: u64,
    closed: bool,
    suspended: bool,
    enabled: bool,
    active: Option<Active>,
}

pub struct Manager {
    journal: Result<PathBuf, String>,
    state: Mutex<State>,
}
impl Manager {
    pub fn new(data_dir: &Path) -> Self {
        let mut session = 0;
        let journal = unsafe {
            windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                std::process::id(),
                &mut session,
            )
        }
        .map(|()| data_dir.join(format!("communications-route-{session}.json")))
        .map_err(|e| e.to_string());
        Self {
            journal,
            state: Mutex::new(State::default()),
        }
    }
    fn journal(&self) -> Result<&Path, String> {
        self.journal.as_deref().map_err(Clone::clone)
    }
    pub fn recover(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if state.active.is_some() {
            return Ok(());
        }
        route::recover(self.journal()?, &WindowsDefaults)
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.state.lock().unwrap().enabled = enabled;
        if enabled {
            Ok(())
        } else {
            self.end(None, false)
        }
    }
    pub(super) fn check_available(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if !state.enabled {
            return Err("Virtual microphone routing is turned off.".into());
        }
        if state.suspended || state.closed {
            return Err(
                "Virtual microphone routing requires an active, unlocked desktop session.".into(),
            );
        }
        WindowsDefaults.target()?;
        communications::cable_endpoint(eRender)?;
        WindowsDefaults.current()?;
        Ok(())
    }
    fn begin(&self) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("Dictto is shutting down.".into());
        }
        if state.suspended {
            return Err(
                "Virtual microphone routing requires an active, unlocked desktop session.".into(),
            );
        }
        if state.active.is_some() {
            return Err("Another virtual microphone action is still active.".into());
        }
        if !state.enabled {
            return Err("Virtual microphone routing was turned off.".into());
        }
        let target = route::acquire(self.journal()?, &WindowsDefaults)?;
        state.generation += 1;
        let token = state.generation;
        state.active = Some(Active {
            token,
            target,
            checked: Instant::now(),
        });
        Ok(token)
    }
    fn check_route(&self, token: u64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let active = state
            .active
            .as_mut()
            .filter(|a| a.token == token)
            .ok_or("Virtual microphone playback was stopped.")?;
        if active.checked.elapsed() >= Duration::from_millis(100) {
            if WindowsDefaults.current()? != active.target {
                return Err(
                    "The communications microphone changed. Dictto stopped routing speech.".into(),
                );
            }
            active.checked = Instant::now();
        }
        Ok(())
    }
    fn end(&self, token: Option<u64>, closed: bool) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.closed |= closed;
        if state
            .active
            .as_ref()
            .is_none_or(|a| token.is_some_and(|t| t != a.token))
        {
            return Ok(());
        }
        state.active = None;
        route::recover(self.journal()?, &WindowsDefaults)
    }
    pub fn session_changed(&self, suspended: bool) -> Result<(), String> {
        self.state.lock().unwrap().suspended = suspended;
        if suspended {
            self.end(None, false)
        } else {
            self.recover()
        }
    }
    pub fn shutdown(&self) {
        if let Err(error) = self.end(None, true) {
            log::error!("Communications microphone recovery pending: {error}");
        }
    }
}

pub(super) struct Player {
    manager: Arc<Manager>,
    token: Mutex<Option<u64>>,
}
impl Player {
    pub(super) fn new(manager: Arc<Manager>) -> Self {
        Self {
            manager,
            token: Mutex::new(None),
        }
    }
    pub(super) async fn play<R: Read + Send>(
        &self,
        wav: hound::WavReader<R>,
    ) -> Result<(), String> {
        if wav.duration() == 0 {
            return Err("Empty speech WAV.".into());
        }
        let endpoint = communications::cable_endpoint(eRender)?;
        let device_id = playback::device_id_for_endpoint(&endpoint)?;
        let token = {
            let mut slot = self.token.lock().unwrap();
            let token = self.manager.begin()?;
            *slot = Some(token);
            token
        };
        // Give communications clients time to reopen capture after the default changes.
        tokio::time::sleep(Duration::from_millis(400)).await;
        playback::play_to(wav, device_id, || self.manager.check_route(token)).await?;
        // WinMM completion can precede delivery of the last samples at the capture end.
        tokio::time::sleep(Duration::from_millis(250)).await;
        self.manager.check_route(token)
    }
    pub(super) fn finish(&self) -> Result<(), String> {
        let token = self.token.lock().unwrap().take();
        if let Some(value) = token {
            self.manager.end(Some(value), false)
        } else {
            Ok(())
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            log::error!("Microphone restoration pending: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires an installed VB-CABLE and temporarily changes the communications mic"]
    async fn cable_playback_restores_the_communications_input() {
        let folder = std::env::temp_dir().join(format!("dictto-cable-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let manager = Arc::new(Manager::new(&folder));
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
        let player = Player::new(manager);
        let result = player.play(hound::WavReader::new(wav).unwrap()).await;
        let during = WindowsDefaults.current();
        player.finish().unwrap();
        assert_eq!(WindowsDefaults.current().unwrap(), original);
        result.unwrap();
        assert_eq!(during.unwrap(), target);
        std::fs::remove_dir(folder).unwrap();
    }
}
