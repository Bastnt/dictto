//! The settings process reaches the database and the keyring only through the daemon's
//! com.dictto.App1 (apps/gnome/dbus/com.dictto.App1.xml). zbus runs on its own tokio
//! runtime; the GTK thread awaits the results.
use futures_util::StreamExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use zbus::fdo;

pub(crate) const APP_BUS_NAME: &str = "com.dictto.App";
/// Long enough for a keyring unlock prompt behind SaveKey.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const NO_ANSWER: &str = "Dictto did not answer.";

#[zbus::proxy(
    interface = "com.dictto.App1",
    default_service = "com.dictto.App",
    default_path = "/com/dictto/App"
)]
trait App {
    fn set_hotkey_capture(&self, active: bool) -> zbus::Result<()>;
    fn set_setting(&self, key: &str, value: &str) -> zbus::Result<String>;
    fn get_settings(&self) -> zbus::Result<HashMap<String, String>>;
    fn save_key(&self, key: &str) -> zbus::Result<String>;
    fn remove_key(&self) -> zbus::Result<String>;
    fn key_hint(&self) -> zbus::Result<String>;
    fn microphones(&self) -> zbus::Result<Vec<(String, String, String, bool)>>;
    fn discover_speech_models(&self, generation: u64) -> zbus::Result<()>;
    fn discover_models(&self, generation: u64, refresh: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn event(&self, name: String, payload_json: String) -> zbus::Result<()>;
    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;
}

pub(crate) enum Incoming {
    /// Whether `com.dictto.App` has an owner; sent once on connect, then on every change.
    Present(bool),
    Event(String, String),
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("dictto-settings-bus")
            .enable_all()
            .build()
            .expect("build the D-Bus runtime")
    })
}

/// Runs `future` on the bus runtime; the returned future can be awaited from GTK.
fn on_bus<T: Send + 'static>(
    future: impl Future<Output = zbus::Result<T>> + Send + 'static,
) -> impl Future<Output = Result<T, String>> {
    let task = runtime().spawn(async move { tokio::time::timeout(CALL_TIMEOUT, future).await });
    async move {
        match task.await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(describe(error)),
            Ok(Err(_)) | Err(_) => Err(NO_ANSWER.into()),
        }
    }
}

/// The daemon answers failures with org.freedesktop.DBus.Error.Failed and readable text.
fn describe(error: zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(message), _) => message,
        zbus::Error::FDO(error) => match *error {
            fdo::Error::Failed(message) => message,
            other => format!("Cannot reach Dictto: {other}"),
        },
        other => format!("Cannot reach Dictto: {other}"),
    }
}

/// The daemon reports setting and key errors as text, empty on success.
fn text_result(text: String) -> Result<(), String> {
    if text.is_empty() {
        Ok(())
    } else {
        Err(text)
    }
}

#[derive(Clone)]
pub(crate) struct Client {
    proxy: AppProxy<'static>,
    capture: Arc<watch::Sender<bool>>,
    applied: watch::Receiver<bool>,
}

impl Client {
    /// Connects to the session bus; the daemon need not be running.
    pub async fn connect() -> Result<(Client, mpsc::UnboundedReceiver<Incoming>), String> {
        on_bus(async {
            let connection = zbus::Connection::session().await?;
            let proxy = AppProxy::builder(&connection)
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await?;
            let dbus = fdo::DBusProxy::new(&connection).await?;
            let mut owners = dbus.receive_name_owner_changed_with_args(&[(0, APP_BUS_NAME)]).await?;
            let mut events = proxy.receive_event().await?;
            let present = dbus.name_has_owner(APP_BUS_NAME.try_into()?).await?;
            let (incoming, received) = mpsc::unbounded_channel();
            let _ = incoming.send(Incoming::Present(present));
            let owner_sink = incoming.clone();
            tokio::spawn(async move {
                while let Some(change) = owners.next().await {
                    if let Ok(args) = change.args() {
                        let _ = owner_sink.send(Incoming::Present(args.new_owner().is_some()));
                    }
                }
            });
            tokio::spawn(async move {
                while let Some(signal) = events.next().await {
                    let Ok(args) = signal.args() else { continue };
                    // The recording bar's frames arrive many times a second; no page uses them.
                    if args.name != dictto_core::events::WAVEFORM_DATA {
                        let _ = incoming.send(Incoming::Event(args.name, args.payload_json));
                    }
                }
            });
            // One task sends capture changes in order, coalescing quick focus moves.
            let (capture, mut wanted) = watch::channel(false);
            let (done, applied) = watch::channel(false);
            let capture_proxy = proxy.clone();
            tokio::spawn(async move {
                while wanted.changed().await.is_ok() {
                    let active = *wanted.borrow_and_update();
                    if let Err(error) = capture_proxy.set_hotkey_capture(active).await {
                        log::warn!("cannot set shortcut capture: {error}");
                    }
                    done.send_replace(active);
                }
            });
            Ok((Client { proxy, capture: Arc::new(capture), applied }, received))
        })
        .await
    }

    /// Suspends the daemon's shortcuts while a shortcut field records keys.
    pub fn set_capture(&self, active: bool) {
        self.capture.send_if_modified(|current| std::mem::replace(current, active) != active);
    }

    /// Blocks until capture is off at the daemon, or a second has passed; for window close.
    pub fn release_capture(&self) {
        self.set_capture(false);
        let mut applied = self.applied.clone();
        runtime().block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(1), applied.wait_for(|active| !*active)).await;
        });
    }

    pub async fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.get_settings().await }).await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let (proxy, key, value) = (self.proxy.clone(), key.to_owned(), value.to_owned());
        on_bus(async move { proxy.set_setting(&key, &value).await }).await.and_then(text_result)
    }

    pub async fn save_key(&self, key: &str) -> Result<(), String> {
        let (proxy, key) = (self.proxy.clone(), key.to_owned());
        on_bus(async move { proxy.save_key(&key).await }).await.and_then(text_result)
    }

    pub async fn remove_key(&self) -> Result<(), String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.remove_key().await }).await.and_then(text_result)
    }

    pub async fn key_hint(&self) -> Result<String, String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.key_hint().await }).await
    }

    pub async fn microphones(&self) -> Result<Vec<(String, String, String, bool)>, String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.microphones().await }).await
    }

    pub async fn status(&self) -> Result<String, String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.status().await }).await
    }

    pub async fn discover_speech_models(&self, generation: u64) -> Result<(), String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.discover_speech_models(generation).await }).await
    }

    /// The answer arrives as the `model-catalog` event carrying `generation`.
    pub async fn discover_models(&self, generation: u64, refresh: bool) -> Result<(), String> {
        let proxy = self.proxy.clone();
        on_bus(async move { proxy.discover_models(generation, refresh).await }).await
    }
}
