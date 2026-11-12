//! `com.dictto.App1`: the service the indicator, the recording bar and the settings
//! process talk to. Bodies sit behind `AppBackend` so tests need no runtime.
use super::{APP_BUS_NAME, APP_PATH};
use dictto_core::audio::MicrophoneInfo;
use dictto_core::host::async_runtime::{self, JoinHandle};
use dictto_core::events;
use dictto_core::Runtime;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};
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
type Job<B> = Box<dyn FnOnce(&B) + Send>;

pub struct AppService<B: AppBackend> {
    backend: Arc<B>,
    /// One thread runs every blocking backend call in arrival order, so close calls such
    /// as two SetSetting cannot overtake each other. The price: a keyring call that hangs
    /// (an unanswered unlock prompt) holds back later settings calls; accepted, since
    /// they would race otherwise.
    worker: std::sync::mpsc::Sender<Job<B>>,
    show_settings: Callback,
    quit: Callback,
    status: Arc<Mutex<String>>,
}

impl<B: AppBackend> AppService<B> {
    /// `show_settings` and `quit` run on the bus dispatcher and must not block.
    pub fn new(
        backend: Arc<B>,
        show_settings: impl Fn() + Send + Sync + 'static,
        quit: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (worker, jobs) = std::sync::mpsc::channel::<Job<B>>();
        let thread_backend = backend.clone();
        std::thread::Builder::new()
            .name("dictto-dbus-worker".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    job(&thread_backend);
                }
            })
            .expect("spawn D-Bus worker thread");
        Self {
            backend,
            worker,
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
        let (reply, answer) = oneshot::channel();
        let job: Job<B> = Box::new(move |backend| {
            let _ = reply.send(work(backend));
        });
        self.worker.send(job).map_err(|_| fdo::Error::Failed("worker stopped".into()))?;
        answer.await.map_err(|_| fdo::Error::Failed("worker stopped".into()))
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

    // The bar, capture and discovery calls only flip state or spawn, so they run inline
    // and keep call order.
    fn bar_start(&self) {
        self.backend.start_recording();
    }

    fn bar_stop(&self) {
        self.backend.stop_recording();
    }

    fn bar_cancel(&self) {
        self.backend.cancel_recording();
    }

    fn set_hotkey_capture(&self, active: bool) {
        self.backend.set_hotkey_capture(active);
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

    fn discover_speech_models(&self, generation: u64) {
        self.backend.discover_speech_models(generation);
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, name: &str, payload_json: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

pub enum Claim<B: AppBackend> {
    Owner(Served<B>),
    /// Another daemon owns the name: call `request_settings` and exit.
    Secondary,
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
        Err(zbus::Error::NameTaken) => Ok(Claim::Secondary),
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
            let mut last_warn: Option<Instant> = None;
            let mut suppressed = 0u32;
            let mut dropped_seen = 0u64;
            let mut last_drop_log: Option<Instant> = None;
            while let Some((name, payload)) = events.recv().await {
                let dropped = events.dropped();
                if dropped > dropped_seen && last_drop_log.is_none_or(|t| t.elapsed() >= WARN_INTERVAL) {
                    log::warn!("event queue full: {} event(s) dropped ({dropped} in total)", dropped - dropped_seen);
                    dropped_seen = dropped;
                    last_drop_log = Some(Instant::now());
                }
                if let Err(error) =
                    AppService::<B>::event(interface.signal_emitter(), &name, &payload).await
                {
                    if last_warn.is_none_or(|t| t.elapsed() >= WARN_INTERVAL) {
                        log::warn!("cannot emit event {name}: {error} ({suppressed} similar suppressed)");
                        last_warn = Some(Instant::now());
                        suppressed = 0;
                    } else {
                        suppressed += 1;
                    }
                }
            }
        })
    }
}

const WARN_INTERVAL: Duration = Duration::from_secs(10);
/// Pending events before the oldest start being dropped; state events are rare, so
/// reaching this means nobody drains the queue.
const QUEUE_CAPACITY: usize = 256;

#[derive(Default)]
struct Queue {
    pending: VecDeque<(String, String)>,
    dropped: u64,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Notify,
}

pub struct EventReceiver {
    shared: Arc<Shared>,
}

impl EventReceiver {
    pub async fn recv(&mut self) -> Option<(String, String)> {
        loop {
            {
                let mut queue = self.shared.queue.lock().ok()?;
                if let Some(event) = queue.pending.pop_front() {
                    return Some(event);
                }
                if queue.closed {
                    return None;
                }
            }
            self.shared.ready.notified().await;
        }
    }

    /// Events discarded because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.shared.queue.lock().map(|q| q.dropped).unwrap_or(0)
    }
}

struct SinkHandle(Arc<Shared>);

impl SinkHandle {
    fn push(&self, name: &str, payload: String) {
        let Ok(mut queue) = self.0.queue.lock() else { return };
        let entry = (name.to_string(), payload);
        if name == events::WAVEFORM_DATA {
            // Only the newest frame matters; an older pending one is replaced in place.
            if let Some(slot) = queue.pending.iter_mut().find(|(n, _)| n == events::WAVEFORM_DATA) {
                *slot = entry;
                drop(queue);
                self.0.ready.notify_one();
                return;
            }
        }
        if queue.pending.len() >= QUEUE_CAPACITY {
            queue.dropped += 1;
            // A waveform frame is the cheapest loss: an incoming one is dropped rather
            // than evicting a state event, and none can be pending here (it would have
            // been replaced above). Otherwise the oldest event goes.
            if name == events::WAVEFORM_DATA {
                return;
            }
            queue.pending.pop_front();
        }
        queue.pending.push_back(entry);
        drop(queue);
        self.0.ready.notify_one();
    }
}

impl Drop for SinkHandle {
    fn drop(&mut self) {
        if let Ok(mut queue) = self.0.queue.lock() {
            queue.closed = true;
        }
        self.0.ready.notify_one();
    }
}

/// The sink handed to `Runtime::new`. It only queues into a bounded buffer, so any core
/// thread can call it without touching the bus or blocking; `Served::forward_events`
/// drains the queue once connected.
pub fn event_sink() -> (impl Fn(&str, serde_json::Value) + Send + Sync + 'static, EventReceiver) {
    let shared = Arc::new(Shared { queue: Mutex::new(Queue::default()), ready: Notify::new() });
    let handle = SinkHandle(shared.clone());
    let sink = move |name: &str, payload: serde_json::Value| {
        handle.push(name, friendly_payload(name, payload).to_string())
    };
    (sink, EventReceiver { shared })
}

/// The extension shows `dictation-error` messages verbatim, so they are made readable
/// here, as the Windows overlay does before display. The raw error is already logged.
fn friendly_payload(name: &str, mut payload: serde_json::Value) -> serde_json::Value {
    if name == events::DICTATION_ERROR {
        if let Some(message) = payload.get_mut("message") {
            if let Some(raw) = message.as_str() {
                *message = dictto_core::transcription::friendly_error(raw).into();
            }
        }
    }
    payload
}

#[cfg(test)]
mod tests;
