//! `com.dictto.App1`: the service the indicator, the recording bar and the settings
//! process talk to. Bodies sit behind `AppBackend` so tests need no runtime.
use super::{APP_BUS_NAME, APP_PATH};
use dictto_core::audio::MicrophoneInfo;
use dictto_core::host::async_runtime::{self, JoinHandle};
use dictto_core::Runtime;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use zbus::fdo;
use zbus::object_server::{InterfaceRef, SignalEmitter};

pub trait AppBackend: Send + Sync + 'static {
    fn set_setting(&self, key: &str, value: &str) -> Result<(), String>;
    fn get_settings(&self) -> Result<HashMap<String, String>, String>;
    fn save_key(&self, key: &str) -> Result<(), String>;
    fn remove_key(&self) -> Result<(), String>;
    fn key_hint(&self) -> Result<String, String>;
    fn microphones(&self) -> Result<Vec<MicrophoneInfo>, String>;
    fn set_hotkey_capture(&self, active: bool);
    fn start_recording(&self);
    fn stop_recording(&self);
    fn cancel_recording(&self);
    fn discover_speech_models(&self, generation: u64);
}

impl AppBackend for Runtime {
    fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        Runtime::set_setting(self, key, value)
    }
    fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        Runtime::get_settings(self)
    }
    fn save_key(&self, key: &str) -> Result<(), String> {
        Runtime::save_key(self, key)
    }
    fn remove_key(&self) -> Result<(), String> {
        Runtime::remove_key(self)
    }
    fn key_hint(&self) -> Result<String, String> {
        Runtime::key_hint(self)
    }
    fn microphones(&self) -> Result<Vec<MicrophoneInfo>, String> {
        Runtime::microphones(self)
    }
    fn set_hotkey_capture(&self, active: bool) {
        Runtime::set_hotkey_capture(self, active);
    }
    fn start_recording(&self) {
        Runtime::start_recording(self);
    }
    fn stop_recording(&self) {
        Runtime::stop_recording(self);
    }
    fn cancel_recording(&self) {
        Runtime::cancel_recording(self);
    }
    fn discover_speech_models(&self, generation: u64) {
        Runtime::discover_speech_models(self, generation);
    }
}

type Callback = Box<dyn Fn() + Send + Sync>;

pub struct AppService<B: AppBackend> {
    backend: Arc<B>,
    show_settings: Callback,
    quit: Callback,
    status: Arc<Mutex<String>>,
}

impl<B: AppBackend> AppService<B> {
    pub fn new(
        backend: Arc<B>,
        show_settings: impl Fn() + Send + Sync + 'static,
        quit: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            backend,
            show_settings: Box::new(show_settings),
            quit: Box::new(quit),
            status: Arc::new(Mutex::new("ok".into())),
        }
    }

    /// Keyring and database work must not stall the bus dispatcher.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&B) -> T + Send + 'static,
    ) -> fdo::Result<T> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || work(&backend))
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

fn error_text(result: Result<(), String>) -> String {
    result.err().unwrap_or_default()
}

#[zbus::interface(name = "com.dictto.App1")]
impl<B: AppBackend> AppService<B> {
    fn show_settings(&self) {
        (self.show_settings)();
    }

    fn quit(&self) {
        (self.quit)();
    }

    async fn bar_start(&self) -> fdo::Result<()> {
        self.blocking(|b| b.start_recording()).await
    }

    async fn bar_stop(&self) -> fdo::Result<()> {
        self.blocking(|b| b.stop_recording()).await
    }

    async fn bar_cancel(&self) -> fdo::Result<()> {
        self.blocking(|b| b.cancel_recording()).await
    }

    async fn set_hotkey_capture(&self, active: bool) -> fdo::Result<()> {
        self.blocking(move |b| b.set_hotkey_capture(active)).await
    }

    async fn set_setting(&self, key: String, value: String) -> fdo::Result<String> {
        self.blocking(move |b| error_text(b.set_setting(&key, &value))).await
    }

    async fn get_settings(&self) -> fdo::Result<HashMap<String, String>> {
        self.blocking(|b| b.get_settings()).await?.map_err(fdo::Error::Failed)
    }

    async fn save_key(&self, key: String) -> fdo::Result<String> {
        self.blocking(move |b| error_text(b.save_key(&key))).await
    }

    async fn remove_key(&self) -> fdo::Result<String> {
        self.blocking(|b| error_text(b.remove_key())).await
    }

    async fn key_hint(&self) -> fdo::Result<String> {
        self.blocking(|b| b.key_hint().unwrap_or_default()).await
    }

    async fn microphones(&self) -> fdo::Result<Vec<(String, String, String, bool)>> {
        let list = self.blocking(|b| b.microphones()).await?.map_err(fdo::Error::Failed)?;
        Ok(list
            .into_iter()
            .map(|m| (m.id, m.name, m.form_factor, m.is_default))
            .collect())
    }

    async fn discover_speech_models(&self, generation: u64) -> fdo::Result<()> {
        self.blocking(move |b| b.discover_speech_models(generation)).await
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, name: &str, payload_json: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// What a process does when it starts and finds the bus name taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    ShowSettingsAndExit,
}

pub fn on_name_taken() -> Action {
    Action::ShowSettingsAndExit
}

pub enum Claim<B: AppBackend> {
    Owner(Served<B>),
    /// Another daemon owns the name; see `request_settings`.
    Secondary(Action),
}

/// Registered service plus the handles needed to publish through it.
pub struct Served<B: AppBackend> {
    connection: zbus::Connection,
    interface: InterfaceRef<AppService<B>>,
    status: Arc<Mutex<String>>,
}

/// Claims `com.dictto.App` on the session bus. Must run inside the core runtime so
/// zbus' tasks live there.
pub async fn claim<B: AppBackend>(service: AppService<B>) -> zbus::Result<Claim<B>> {
    let status = service.status.clone();
    // zbus defaults to replacing the current owner, which would let a second daemon steal
    // the name instead of detecting it.
    let built = zbus::connection::Builder::session()?
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .name(APP_BUS_NAME)?
        .serve_at(APP_PATH, service)?
        .build()
        .await;
    match built {
        Ok(connection) => {
            let interface = connection.object_server().interface(APP_PATH).await?;
            Ok(Claim::Owner(Served { connection, interface, status }))
        }
        Err(zbus::Error::NameTaken) => Ok(Claim::Secondary(on_name_taken())),
        Err(error) => Err(error),
    }
}

#[zbus::proxy(
    interface = "com.dictto.App1",
    default_service = "com.dictto.App",
    default_path = "/com/dictto/App"
)]
trait RunningApp {
    fn show_settings(&self) -> zbus::Result<()>;
}

/// Asks the already running daemon to open its settings window.
pub async fn request_settings() -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    RunningAppProxy::new(&connection).await?.show_settings().await
}

impl<B: AppBackend> Served<B> {
    pub fn connection(&self) -> &zbus::Connection {
        &self.connection
    }

    /// Sets the `Status` property and notifies watchers; "ok" means no problem.
    pub async fn set_status(&self, text: &str) -> zbus::Result<()> {
        match self.status.lock() {
            Ok(mut status) if *status != text => *status = text.to_string(),
            _ => return Ok(()),
        }
        self.interface.get().await.status_changed(self.interface.signal_emitter()).await
    }

    /// Emits every queued core event as the `Event` signal until the sink is dropped.
    pub fn forward_events(&self, mut events: EventReceiver) -> JoinHandle<()> {
        let interface = self.interface.clone();
        async_runtime::spawn(async move {
            while let Some((name, payload)) = events.recv().await {
                if let Err(error) =
                    AppService::<B>::event(interface.signal_emitter(), &name, &payload).await
                {
                    log::warn!("cannot emit event {name}: {error}");
                }
            }
        })
    }
}

pub struct EventReceiver {
    rx: UnboundedReceiver<(String, String)>,
}

impl EventReceiver {
    pub async fn recv(&mut self) -> Option<(String, String)> {
        self.rx.recv().await
    }
}

/// The sink handed to `Runtime::new`. It only queues, so any core thread can call it
/// without touching the bus; `Served::forward_events` drains the queue once connected.
pub fn event_sink() -> (impl Fn(&str, serde_json::Value) + Send + Sync + 'static, EventReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let sink = move |name: &str, payload: serde_json::Value| {
        let _ = tx.send((name.to_string(), payload.to_string()));
    };
    (sink, EventReceiver { rx })
}

#[cfg(test)]
mod tests;
