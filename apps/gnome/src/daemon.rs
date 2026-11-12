//! The daemon: owns the shared runtime, serves `com.dictto.App` and follows the session.
use crate::audio::{self, Pipewire, PipewireDevices, PipewireRecorder, PipewireSounds};
use crate::dbus::app::{self, AppBackend, AppService, Claim, Served};
use crate::dbus::{shell, APP_BUS_NAME};
use crate::injector::{DesktopReporter, GnomeInjector, UinputKeys};
use crate::keyboard::{self, KeyboardError, KeyboardHandle};
use crate::session::{self, SessionSink};
use crate::Mode;
use dictto_core::audio::{AudioError, DeviceMonitorState, DeviceSelection, MicrophoneInfo};
use dictto_core::host::{async_runtime, AppHandle};
use dictto_core::platform::{Devices, Platform, Recorder, Services, Sound, Sounds, Wording};
use dictto_core::Runtime;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{watch, Notify};

pub const WORDING: Wording = Wording {
    no_microphone: "No microphone is available. Connect one, then try again.",
    microphone_unavailable:
        "Cannot open the microphone. Check the input device in Settings > Audio and that PipeWire is running.",
    credential_store: "GNOME Keyring",
};

const KEYBOARD_DENIED: &str =
    "Dictto cannot read the keyboard: add your user to the input group and log in again";
const KEYBOARD_DENIED_HELP: &str =
    "Run \"sudo usermod -aG input $USER\", then log out and back in. Dictation still works from the recording bar.";
const AUDIO_UNAVAILABLE: &str = "Dictto cannot reach PipeWire";
const PREVIEW: &str = "Preview: recording services disabled";

pub fn run(mode: Mode) -> Result<(), String> {
    async_runtime::initialize()?;
    let smoke = mode == Mode::SmokeTest;
    let (sink, events) = app::event_sink();
    let backend = Arc::new(LateRuntime::default());
    let quit = Arc::new(Notify::new());
    let quit_request = quit.clone();
    let service = AppService::new(backend.clone(), open_settings, move || quit_request.notify_one());

    // Claimed before anything opens keyboards or PipeWire, so a second start only asks
    // the running daemon for its settings.
    let served = match block_on(app::claim(service))?.map_err(|e| format!("Cannot use the session bus: {e}"))? {
        Claim::Owner(served) => Arc::new(served),
        Claim::Secondary => {
            log::info!("Dictto is already running; opening its settings");
            return block_on(app::request_settings())?.map_err(|e| e.to_string());
        }
    };
    let (terminate, interrupt) = block_on(async {
        Ok::<_, std::io::Error>((signal(SignalKind::terminate())?, signal(SignalKind::interrupt())?))
    })?
    .map_err(|e| format!("Cannot handle signals: {e}"))?;

    let board = StatusBoard::new();
    async_runtime::spawn(board.clone().publish(served.clone()));
    // Without libpipewire's client setup the daemon still serves settings and reports
    // the problem; only a restart retries.
    let pipewire = match Pipewire::shared() {
        Ok(pipewire) => Some(pipewire),
        Err(error) => {
            log::error!("PipeWire is unavailable: {error}");
            board.set(Slot::Audio, Some(AUDIO_UNAVAILABLE));
            if !smoke {
                notify("Dictto cannot use the microphone", format!("{AUDIO_UNAVAILABLE}. {error}"));
            }
            None
        }
    };
    let connection = served.connection().clone();
    let shell = block_on(async move { shell::connect(&connection).await })?.map_err(|e| e.to_string())?;
    let paste_board = board.clone();
    let reporter = DesktopReporter::new(move |text| paste_board.set(Slot::Paste, (text != "ok").then_some(text)));
    let services = Arc::new(GnomeServices {
        pipewire: pipewire.clone(),
        keyboard: Mutex::new(None),
        board: board.clone(),
        notified: AtomicBool::new(false),
    });
    let data_dir = crate::paths::data_dir();
    // Without PipeWire nothing can be spoken, so the runtime is built without speech.
    let speech = pipewire
        .as_ref()
        .map(|pipewire| crate::speech::platform(pipewire.clone(), shell.clone(), &data_dir));
    let injector = Arc::new(GnomeInjector::new(shell, UinputKeys::default(), reporter));
    let platform = match pipewire {
        Some(pipewire) => Platform {
            injector,
            sounds: Arc::new(PipewireSounds::new(pipewire.clone())),
            devices: Arc::new(PipewireDevices::new(pipewire.clone())),
            services: services.clone(),
            new_recorder: Arc::new(move || Box::new(PipewireRecorder::new(pipewire.clone()))),
            wording: &WORDING,
        },
        None => Platform {
            injector,
            sounds: Arc::new(NoAudio),
            devices: Arc::new(NoAudio),
            services: services.clone(),
            new_recorder: Arc::new(|| Box::new(NoAudio)),
            wording: &WORDING,
        },
    };
    let runtime = match speech {
        Some(speech) => Runtime::with_speech(data_dir, platform, speech, sink)?,
        None => Runtime::new(data_dir, platform, sink)?,
    };
    let _ = backend.0.set(runtime.clone());
    let forwarder = served.forward_events(events);

    let starter = Starter { runtime: runtime.clone(), services: services.clone(), board: board.clone() };
    let watcher = if smoke {
        board.set(Slot::Start, Some(PREVIEW));
        None
    } else {
        let watcher = session::watch(starter.clone());
        starter.start();
        Some(watcher)
    };
    if mode != Mode::Autostart || board.has_problem() {
        open_settings();
    }

    block_on(wait_for_quit(quit, terminate, interrupt))?;
    log::info!("shutting down");
    if let Some(watcher) = watcher {
        watcher.abort();
    }
    runtime.shutdown();
    services.close_keyboard();
    forwarder.abort();
    let connection = served.connection().clone();
    if let Err(error) = block_on(async move { connection.release_name(APP_BUS_NAME).await })? {
        log::warn!("cannot release {APP_BUS_NAME}: {error}");
    }
    Ok(())
}

async fn wait_for_quit(
    quit: Arc<Notify>,
    mut terminate: tokio::signal::unix::Signal,
    mut interrupt: tokio::signal::unix::Signal,
) {
    tokio::select! {
        _ = quit.notified() => log::info!("quit requested over D-Bus"),
        _ = terminate.recv() => log::info!("SIGTERM received"),
        _ = interrupt.recv() => log::info!("SIGINT received"),
    }
}

/// Runs `future` on the core runtime and waits for it from this (non-runtime) thread.
fn block_on<F>(future: F) -> Result<F::Output, String>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (reply, answer) = std::sync::mpsc::channel();
    async_runtime::spawn(async move {
        let _ = reply.send(future.await);
    });
    answer.recv().map_err(|_| "a startup task panicked".to_string())
}

/// Spawns the settings process; a second one hands over to the first through its own
/// GApplication uniqueness. Called on the bus dispatcher, so it only forks.
fn open_settings() {
    let mut command = match crate::settings_command() {
        Ok(command) => command,
        Err(error) => return log::error!("{error}"),
    };
    match command.spawn() {
        // Reaped on its own thread so it never lingers as a zombie.
        Ok(mut child) => {
            let _ = std::thread::Builder::new()
                .name("settings-reaper".into())
                .spawn(move || child.wait());
        }
        Err(error) => log::error!("cannot open the settings: {error}"),
    }
}

/// The D-Bus service exists before the runtime (claimed first), so calls arriving in
/// between get an error instead of reaching a runtime that is not built yet.
#[derive(Default)]
struct LateRuntime(OnceLock<Runtime>);

const STARTING: &str = "Dictto is still starting";

impl LateRuntime {
    fn with<T>(&self, call: impl FnOnce(&Runtime) -> Result<T, String>) -> Result<T, String> {
        self.0.get().ok_or_else(|| STARTING.to_string()).and_then(call)
    }
    fn run(&self, call: impl FnOnce(&Runtime)) {
        if let Some(runtime) = self.0.get() {
            call(runtime);
        }
    }
}

impl AppBackend for LateRuntime {
    fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.with(|r| r.set_setting(key, value))
    }
    fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        self.with(Runtime::get_settings)
    }
    fn save_key(&self, key: &str) -> Result<(), String> {
        self.with(|r| r.save_key(key))
    }
    fn remove_key(&self) -> Result<(), String> {
        self.with(Runtime::remove_key)
    }
    fn key_hint(&self) -> Result<String, String> {
        self.with(Runtime::key_hint)
    }
    fn microphones(&self) -> Result<Vec<MicrophoneInfo>, String> {
        self.with(Runtime::microphones)
    }
    fn set_hotkey_capture(&self, active: bool) {
        self.run(|r| r.set_hotkey_capture(active));
    }
    fn start_recording(&self) {
        self.run(Runtime::start_recording);
    }
    fn stop_recording(&self) {
        self.run(Runtime::stop_recording);
    }
    fn cancel_recording(&self) {
        self.run(Runtime::cancel_recording);
    }
    fn discover_speech_models(&self, generation: u64) {
        self.run(|r| r.discover_speech_models(generation));
    }
}

struct GnomeServices {
    pipewire: Option<Arc<Pipewire>>,
    keyboard: Mutex<Option<KeyboardHandle>>,
    board: StatusBoard,
    notified: AtomicBool,
}

impl GnomeServices {
    /// Opens the keyboards unless already open. A refused /dev/input is reported and not
    /// an error: the runtime would otherwise abort its whole start, taking the recording
    /// bar, the device monitor and warmup down with it, and it never calls
    /// `install_input` again once that succeeded, so `Starter` retries here instead.
    fn open_keyboard(&self) -> Result<(), String> {
        let mut slot = self.keyboard.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Ok(());
        }
        match keyboard::spawn() {
            Ok(handle) => {
                *slot = Some(handle);
                self.board.set(Slot::Input, None);
                Ok(())
            }
            Err(KeyboardError::PermissionDenied) => {
                log::warn!("{KEYBOARD_DENIED}");
                self.board.set(Slot::Input, Some(KEYBOARD_DENIED));
                if !self.notified.swap(true, Ordering::AcqRel) {
                    notify("Dictto cannot read the keyboard", KEYBOARD_DENIED_HELP.into());
                }
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn keyboard_missing(&self) -> bool {
        self.keyboard.lock().map(|k| k.is_none()).unwrap_or(false)
    }

    fn close_keyboard(&self) {
        if let Ok(mut slot) = self.keyboard.lock() {
            slot.take();
        }
    }
}

impl Services for GnomeServices {
    fn set_autostart(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            let exe = crate::current_exe().map_err(|e| e.to_string())?;
            crate::autostart::enable(&exe)
        } else {
            crate::autostart::disable()
        }
    }
    fn install_input(&self) -> Result<(), String> {
        self.open_keyboard()
    }
    fn spawn_device_monitor(
        &self,
        state: Arc<DeviceMonitorState>,
        app: AppHandle,
    ) -> std::sync::mpsc::Sender<()> {
        match &self.pipewire {
            Some(pipewire) => audio::spawn_device_monitor(pipewire, state, app),
            None => std::sync::mpsc::channel().0,
        }
    }
}

/// Stands in for PipeWire when its client library cannot start.
struct NoAudio;

fn no_audio() -> AudioError {
    AudioError::DeviceError(AUDIO_UNAVAILABLE.into())
}

impl Recorder for NoAudio {
    fn set_app_handle(&mut self, _app: AppHandle) {}
    fn start(&mut self, _device: DeviceSelection) -> Result<(), AudioError> {
        Err(no_audio())
    }
    fn stop(&mut self) -> Result<Vec<u8>, AudioError> {
        Err(AudioError::NotRecording)
    }
    fn cancel(&mut self) {}
    fn is_recording(&self) -> bool {
        false
    }
}

impl Sounds for NoAudio {
    fn play(&self, _sound: Sound) {}
}

impl Devices for NoAudio {
    fn list_microphones(&self) -> Result<Vec<MicrophoneInfo>, AudioError> {
        Err(no_audio())
    }
}

fn notify(summary: &'static str, body: String) {
    async_runtime::spawn(async move {
        if let Err(error) = crate::notify::show(summary, &body).await {
            log::warn!("desktop notification failed: {error}");
        }
    });
}

/// Starts the runtime and retries what failed each time the session resumes: the user
/// may have fixed a permission meanwhile.
#[derive(Clone)]
struct Starter {
    runtime: Runtime,
    services: Arc<GnomeServices>,
    board: StatusBoard,
}

impl Starter {
    fn start(&self) {
        match self.runtime.start() {
            Ok(()) => self.board.set(Slot::Start, None),
            Err(error) => {
                log::error!("cannot start dictation: {error}");
                if !self.board.has(Slot::Start) {
                    notify("Dictto cannot start", error.clone());
                }
                self.board.set(Slot::Start, Some(&error));
            }
        }
    }
}

impl SessionSink for Starter {
    fn input_suspended(&self, suspended: bool) {
        self.runtime.input_suspended(suspended);
    }
    fn session_changed(&self, suspended: bool) {
        self.runtime.session_changed(suspended);
        if suspended {
            return;
        }
        if self.board.has(Slot::Start) {
            self.start();
        } else if self.services.keyboard_missing() {
            if let Err(error) = self.services.open_keyboard() {
                log::warn!("cannot open the keyboards: {error}");
            }
        }
    }
}

/// Sources of the App1 `Status` property, highest priority first.
#[derive(Clone, Copy)]
enum Slot {
    Start,
    Input,
    Audio,
    Paste,
}

/// Combines the problems so one source clearing its own (a successful paste) does not
/// hide another's (no keyboard access).
#[derive(Clone)]
struct StatusBoard {
    slots: Arc<Mutex<[Option<String>; 4]>>,
    shown: watch::Sender<String>,
}

impl StatusBoard {
    fn new() -> Self {
        Self { slots: Arc::default(), shown: watch::Sender::new("ok".into()) }
    }

    fn set(&self, slot: Slot, problem: Option<&str>) {
        let Ok(mut slots) = self.slots.lock() else { return };
        slots[slot as usize] = problem.map(str::to_owned);
        let text = slots.iter().flatten().next().cloned().unwrap_or_else(|| "ok".into());
        // Sent under the lock so two setters cannot publish out of order.
        self.shown.send_if_modified(|shown| {
            let changed = *shown != text;
            *shown = text;
            changed
        });
    }

    fn has(&self, slot: Slot) -> bool {
        self.slots.lock().map(|s| s[slot as usize].is_some()).unwrap_or(false)
    }

    fn has_problem(&self) -> bool {
        self.slots.lock().map(|s| s.iter().any(Option::is_some)).unwrap_or(false)
    }

    #[cfg(test)]
    fn text(&self) -> String {
        self.shown.borrow().clone()
    }

    /// Mirrors the board onto App1 `Status`; one task, so updates land in order.
    async fn publish(self, served: Arc<Served<LateRuntime>>) {
        let mut shown = self.shown.subscribe();
        loop {
            let text = shown.borrow_and_update().clone();
            if let Err(error) = served.set_status(&text).await {
                log::warn!("cannot publish the status: {error}");
            }
            if shown.changed().await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_shows_the_highest_priority_problem() {
        let board = StatusBoard::new();
        assert_eq!(board.text(), "ok");
        board.set(Slot::Paste, Some("no extension"));
        board.set(Slot::Input, Some(KEYBOARD_DENIED));
        assert_eq!(board.text(), KEYBOARD_DENIED);
        // A successful paste clears only its own problem.
        board.set(Slot::Paste, None);
        assert_eq!(board.text(), KEYBOARD_DENIED);
        board.set(Slot::Input, None);
        assert_eq!(board.text(), "ok");
        assert!(!board.has_problem());
    }

    #[test]
    fn calls_before_the_runtime_exists_fail_softly() {
        let late = LateRuntime::default();
        assert_eq!(late.get_settings().unwrap_err(), STARTING);
        late.start_recording();
        late.set_hotkey_capture(true);
    }
}
