//! Routes one read to a virtual microphone: the default input is switched for the
//! read, a manual change by the user stops playback, and the route is restored after.
use super::route::{self, Defaults};
use super::{Player, VirtualMic};
use async_trait::async_trait;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Playback side of the virtual microphone.
#[async_trait]
pub trait MicOutput: Send + Sync + 'static {
    /// Resolved playback endpoint, looked up before the default input is switched.
    type Target: Send + Sync;
    /// Fails when the playback side of the virtual microphone is missing.
    fn ensure_present(&self) -> Result<(), String>;
    fn locate(&self) -> Result<Self::Target, String>;
    /// Gives capturing applications time to follow the default input change.
    async fn wait_ready(&self);
    /// Plays the WAV, stopping with `check`'s error as soon as it returns one.
    async fn play<R, C>(&self, wav: hound::WavReader<R>, target: &Self::Target, check: C) -> Result<(), String>
    where
        R: Read + Send,
        C: FnMut() -> Result<(), String> + Send;
}

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

/// Owns the microphone route for this desktop session.
pub struct MicRoute<D, O> {
    journal: Result<PathBuf, String>,
    defaults: D,
    output: O,
    state: Mutex<State>,
}

impl<D: Defaults + Send + Sync + 'static, O: MicOutput> MicRoute<D, O> {
    /// `journal` is per desktop session so concurrent sessions never restore each other's input.
    pub fn new(journal: Result<PathBuf, String>, defaults: D, output: O) -> Self {
        Self {
            journal,
            defaults,
            output,
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
        route::recover(self.journal()?, &self.defaults)
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.state.lock().unwrap().enabled = enabled;
        if enabled {
            Ok(())
        } else {
            self.end(None, false)
        }
    }
    pub fn check_available(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if !state.enabled {
            return Err("Virtual microphone routing is turned off.".into());
        }
        if state.suspended || state.closed {
            return Err(
                "Virtual microphone routing requires an active, unlocked desktop session.".into(),
            );
        }
        self.defaults.target()?;
        self.output.ensure_present()?;
        self.defaults.current()?;
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
        let target = route::acquire(self.journal()?, &self.defaults)?;
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
            if self.defaults.current()? != active.target {
                return Err(format!(
                    "The {} changed. Dictto stopped routing speech.",
                    self.defaults.input_name()
                ));
            }
            active.checked = Instant::now();
        }
        Ok(())
    }
    /// `token` limits the restore to that read, so a stale player never ends a newer route.
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
        route::recover(self.journal()?, &self.defaults)
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
            log::error!("Microphone recovery pending: {error}");
        }
    }
}

struct Handle<D, O>(Arc<MicRoute<D, O>>);

impl<D: Defaults + Send + Sync + 'static, O: MicOutput> VirtualMic for Handle<D, O> {
    fn check_available(&self) -> Result<(), String> {
        self.0.check_available()
    }
    fn player(&self) -> Box<dyn Player + Send> {
        Box::new(RoutedPlayer::new(self.0.clone()))
    }
}

/// The `VirtualMic` a platform puts in `SpeechPlatform`; the app keeps `route` for its lifecycle.
pub fn virtual_mic<D: Defaults + Send + Sync + 'static, O: MicOutput>(
    route: &Arc<MicRoute<D, O>>,
) -> Arc<dyn VirtualMic> {
    Arc::new(Handle(route.clone()))
}

/// Plays one read through the route; dropping it restores the user's input.
pub struct RoutedPlayer<D: Defaults + Send + Sync + 'static, O: MicOutput> {
    route: Arc<MicRoute<D, O>>,
    token: Mutex<Option<u64>>,
}
impl<D: Defaults + Send + Sync + 'static, O: MicOutput> RoutedPlayer<D, O> {
    pub fn new(route: Arc<MicRoute<D, O>>) -> Self {
        Self {
            route,
            token: Mutex::new(None),
        }
    }
    pub async fn play<R: Read + Send>(&self, wav: hound::WavReader<R>) -> Result<(), String> {
        if wav.duration() == 0 {
            return Err("Empty speech WAV.".into());
        }
        let target = self.route.output.locate()?;
        let token = {
            let mut slot = self.token.lock().unwrap();
            let token = self.route.begin()?;
            *slot = Some(token);
            token
        };
        self.route.output.wait_ready().await;
        self.route
            .output
            .play(wav, &target, || self.route.check_route(token))
            .await?;
        // Playback completion can precede delivery of the last samples at the capture end.
        tokio::time::sleep(Duration::from_millis(250)).await;
        self.route.check_route(token)
    }
    pub fn finish(&self) -> Result<(), String> {
        let token = self.token.lock().unwrap().take();
        if let Some(value) = token {
            self.route.end(Some(value), false)
        } else {
            Ok(())
        }
    }
}
impl<D: Defaults + Send + Sync + 'static, O: MicOutput> Drop for RoutedPlayer<D, O> {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            log::error!("Microphone restoration pending: {error}");
        }
    }
}

#[async_trait]
impl<D: Defaults + Send + Sync + 'static, O: MicOutput> Player for RoutedPlayer<D, O> {
    fn requires_complete_audio(&self) -> bool {
        true
    }
    async fn chunk(&self, _: Vec<u8>) -> Result<(), String> {
        Err("The virtual microphone plays complete speech only.".into())
    }
    async fn file(&self, path: &Path) -> Result<(), String> {
        let result = self
            .play(hound::WavReader::open(path).map_err(|e| e.to_string())?)
            .await;
        match (result, self.finish()) {
            (Err(error), Err(restore)) => {
                Err(format!("{error} Microphone restoration pending: {restore}"))
            }
            (result, restore) => result.and(restore),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fake(Arc<Mutex<String>>);
    impl Defaults for Fake {
        fn current(&self) -> Result<String, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn target(&self) -> Result<String, String> {
            Ok("virtual".into())
        }
        fn set(&self, id: &str) -> Result<(), String> {
            *self.0.lock().unwrap() = id.into();
            Ok(())
        }
    }

    /// Plays for `steps` ticks of 30 ms, switching the input to "user" at `switch_at`.
    struct Output {
        input: Arc<Mutex<String>>,
        steps: u32,
        switch_at: Option<u32>,
    }
    #[async_trait]
    impl MicOutput for Output {
        type Target = ();
        fn ensure_present(&self) -> Result<(), String> {
            Ok(())
        }
        fn locate(&self) -> Result<(), String> {
            Ok(())
        }
        async fn wait_ready(&self) {}
        async fn play<R, C>(&self, _: hound::WavReader<R>, _: &(), mut check: C) -> Result<(), String>
        where
            R: Read + Send,
            C: FnMut() -> Result<(), String> + Send,
        {
            for step in 0..self.steps {
                if Some(step) == self.switch_at {
                    *self.input.lock().unwrap() = "user".into();
                }
                check()?;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            Ok(())
        }
    }

    static SERIAL: AtomicU64 = AtomicU64::new(0);
    fn route(steps: u32, switch_at: Option<u32>) -> (Arc<MicRoute<Fake, Output>>, Arc<Mutex<String>>, PathBuf) {
        let input = Arc::new(Mutex::new("usb".to_string()));
        let journal = std::env::temp_dir().join(format!(
            "dictto-mic-route-{}-{}.json",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let output = Output { input: input.clone(), steps, switch_at };
        let route = Arc::new(MicRoute::new(Ok(journal.clone()), Fake(input.clone()), output));
        route.set_enabled(true).unwrap();
        (route, input, journal)
    }
    fn wav() -> hound::WavReader<std::io::Cursor<Vec<u8>>> {
        let mut data = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut data, spec).unwrap();
        writer.write_sample(1i16).unwrap();
        writer.finalize().unwrap();
        data.set_position(0);
        hound::WavReader::new(data).unwrap()
    }

    #[tokio::test]
    async fn routed_read_restores_the_input_afterwards() {
        let (route, input, journal) = route(2, None);
        let player = RoutedPlayer::new(route.clone());
        player.play(wav()).await.unwrap();
        assert_eq!(*input.lock().unwrap(), "virtual");
        player.finish().unwrap();
        assert_eq!(*input.lock().unwrap(), "usb");
        assert!(!journal.exists());
    }

    #[tokio::test]
    async fn manual_change_mid_playback_aborts_and_is_not_overwritten() {
        let (route, input, journal) = route(20, Some(2));
        let player = RoutedPlayer::new(route.clone());
        let error = player.play(wav()).await.unwrap_err();
        assert_eq!(error, "The default microphone changed. Dictto stopped routing speech.");
        player.finish().unwrap();
        assert_eq!(*input.lock().unwrap(), "user");
        assert!(!journal.exists());
    }

    #[tokio::test]
    async fn stale_token_never_ends_the_current_route() {
        let (route, input, _journal) = route(1, None);
        let token = route.begin().unwrap();
        route.end(Some(token + 1), false).unwrap();
        assert_eq!(*input.lock().unwrap(), "virtual");
        route.check_route(token).unwrap();
        route.end(Some(token), false).unwrap();
        assert_eq!(*input.lock().unwrap(), "usb");
        assert_eq!(route.check_route(token).unwrap_err(), "Virtual microphone playback was stopped.");
    }

    #[test]
    fn suspend_ends_the_active_route_and_resume_recovers_a_journal() {
        let (route, input, journal) = route(1, None);
        let token = route.begin().unwrap();
        route.session_changed(true).unwrap();
        assert_eq!(*input.lock().unwrap(), "usb");
        assert!(route.check_route(token).is_err());
        assert!(route.begin().is_err());
        assert!(route.check_available().is_err());
        // A route left behind by a crashed instance is restored once the session resumes.
        std::fs::write(&journal, br#"{"original":"usb","temporary":"virtual"}"#).unwrap();
        *input.lock().unwrap() = "virtual".into();
        route.session_changed(false).unwrap();
        assert_eq!(*input.lock().unwrap(), "usb");
        assert!(!journal.exists());
        route.check_available().unwrap();
    }

    #[test]
    fn recover_is_skipped_while_a_route_is_active() {
        let (route, input, journal) = route(1, None);
        route.begin().unwrap();
        route.recover().unwrap();
        assert_eq!(*input.lock().unwrap(), "virtual");
        assert!(journal.exists());
        route.shutdown();
        assert_eq!(*input.lock().unwrap(), "usb");
        assert_eq!(route.begin().unwrap_err(), "Dictto is shutting down.");
    }

    #[test]
    fn disabled_routing_is_refused_and_ends_the_active_route() {
        let (route, input, _journal) = route(1, None);
        route.begin().unwrap();
        route.set_enabled(false).unwrap();
        assert_eq!(*input.lock().unwrap(), "usb");
        assert_eq!(route.check_available().unwrap_err(), "Virtual microphone routing is turned off.");
        assert_eq!(route.begin().unwrap_err(), "Virtual microphone routing was turned off.");
    }
}
