use super::*;
use crate::dbus::shell::ShellProxy;
use crate::dbus::{SHELL_BUS_NAME, SHELL_PATH};
use dictto_core::events;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    generation: AtomicU64,
}

impl Fake {
    fn note(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }
}

impl AppBackend for Fake {
    fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.note(format!("set {key}={value}"));
        if key == "hotkey" && value == "bad" { Err("invalid chord".into()) } else { Ok(()) }
    }
    fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        Ok(HashMap::from([("language".to_string(), "fr".to_string())]))
    }
    fn save_key(&self, key: &str) -> Result<(), String> {
        self.note(format!("save {key}"));
        Ok(())
    }
    fn remove_key(&self) -> Result<(), String> {
        Err("no key".into())
    }
    fn key_hint(&self) -> Result<String, String> {
        Err("no key".into())
    }
    fn microphones(&self) -> Result<Vec<MicrophoneInfo>, String> {
        Ok(vec![MicrophoneInfo {
            name: "Desk".into(),
            id: "mic-1".into(),
            is_default: true,
            form_factor: "Headset".into(),
        }])
    }
    fn set_hotkey_capture(&self, active: bool) {
        self.note(format!("capture {active}"));
    }
    fn start_recording(&self) {
        self.note("start");
    }
    fn stop_recording(&self) {
        self.note("stop");
    }
    fn cancel_recording(&self) {
        self.note("cancel");
    }
    fn discover_speech_models(&self, generation: u64) {
        self.generation.store(generation, Ordering::SeqCst);
    }
}

fn service(fake: &Arc<Fake>) -> AppService<Fake> {
    AppService::new(fake.clone(), || {}, || {})
}

#[tokio::test]
async fn event_signal_carries_json_payload() {
    let (sink, mut events) = event_sink();
    sink(events::RECORDING_STATE_CHANGED, json!({ "state": "recording", "level": 3 }));
    let (name, payload) = events.recv().await.unwrap();
    assert_eq!(name, "recording-state-changed");
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value, json!({ "state": "recording", "level": 3 }));
}

#[test]
fn second_instance_requests_settings() {
    assert_eq!(on_name_taken(), Action::ShowSettingsAndExit);
}

#[tokio::test]
async fn setting_errors_come_back_as_text() {
    let fake = Arc::new(Fake::default());
    let service = service(&fake);
    assert_eq!(service.set_setting("hotkey".into(), "ctrl+x".into()).await.unwrap(), "");
    assert_eq!(service.set_setting("hotkey".into(), "bad".into()).await.unwrap(), "invalid chord");
    assert_eq!(service.remove_key().await.unwrap(), "no key");
    assert_eq!(service.key_hint().await.unwrap(), "");
}

#[tokio::test]
async fn microphones_flatten_in_documented_order() {
    let fake = Arc::new(Fake::default());
    let list = service(&fake).microphones().await.unwrap();
    assert_eq!(list, vec![("mic-1".into(), "Desk".into(), "Headset".into(), true)]);
}

#[tokio::test]
async fn bar_and_capture_reach_the_backend() {
    let fake = Arc::new(Fake::default());
    let service = service(&fake);
    service.bar_start().await.unwrap();
    service.bar_stop().await.unwrap();
    service.bar_cancel().await.unwrap();
    service.set_hotkey_capture(true).await.unwrap();
    service.discover_speech_models(7).await.unwrap();
    assert_eq!(*fake.calls.lock().unwrap(), ["start", "stop", "cancel", "capture true"]);
    assert_eq!(fake.generation.load(Ordering::SeqCst), 7);
}

struct FakeShell;

#[zbus::interface(name = "com.dictto.Shell1")]
impl FakeShell {
    fn save_clipboard(&self) -> u32 {
        41
    }
    fn restore_clipboard(&self, _token: u32) {}
    fn set_text(&self, _text: &str) {}
    fn get_text(&self, which: &str) -> (bool, String) {
        (which == "primary", format!("from {which}"))
    }
}

/// Needs a session bus: `dbus-run-session -- cargo test -p dictto-gnome -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn private_bus_round_trip() {
    use futures_util::StreamExt;
    dictto_core::host::async_runtime::initialize().unwrap();
    let fake = Arc::new(Fake::default());
    let shown = Arc::new(AtomicU64::new(0));
    let shown_by_service = shown.clone();
    let service = AppService::new(
        fake.clone(),
        move || {
            shown_by_service.fetch_add(1, Ordering::SeqCst);
        },
        || {},
    );
    let Claim::Owner(served) = claim(service).await.unwrap() else { panic!("name was free") };
    let (sink, events) = event_sink();
    served.forward_events(events);

    let client = zbus::Connection::session().await.unwrap();
    let proxy = zbus::Proxy::new(&client, APP_BUS_NAME, APP_PATH, "com.dictto.App1").await.unwrap();
    let mut signals = proxy.receive_signal("Event").await.unwrap();
    let mut status = proxy.receive_property_changed::<String>("Status").await;

    sink(events::SETTING_CHANGED, json!({ "key": "language" }));
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), signals.next())
        .await
        .unwrap()
        .unwrap();
    let (name, payload): (String, String) = message.body().deserialize().unwrap();
    assert_eq!(name, "setting-changed");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&payload).unwrap(), json!({ "key": "language" }));

    let error: String = proxy.call("SetSetting", &("hotkey", "bad")).await.unwrap();
    assert_eq!(error, "invalid chord");
    let settings: HashMap<String, String> = proxy.call("GetSettings", &()).await.unwrap();
    assert_eq!(settings["language"], "fr");
    let mics: Vec<(String, String, String, bool)> = proxy.call("Microphones", &()).await.unwrap();
    assert_eq!(mics.len(), 1);
    let () = proxy.call("BarStart", &()).await.unwrap();
    assert_eq!(*fake.calls.lock().unwrap().last().unwrap(), "start");

    served.set_status("Missing input permission").await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), status.next()).await.unwrap().unwrap();
    let uncached = zbus::proxy::Builder::<zbus::Proxy>::new(&client)
        .destination(APP_BUS_NAME)
        .unwrap()
        .path(APP_PATH)
        .unwrap()
        .interface("com.dictto.App1")
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .unwrap();
    assert_eq!(uncached.get_property::<String>("Status").await.unwrap(), "Missing input permission");

    // A second process finds the name taken and asks the first to show its settings.
    let second = claim(AppService::new(fake.clone(), || {}, || {})).await.unwrap();
    assert!(matches!(second, Claim::Secondary(Action::ShowSettingsAndExit)));
    request_settings().await.unwrap();
    assert_eq!(shown.load(Ordering::SeqCst), 1);

    let _shell = zbus::connection::Builder::session()
        .unwrap()
        .name(SHELL_BUS_NAME)
        .unwrap()
        .serve_at(SHELL_PATH, FakeShell)
        .unwrap()
        .build()
        .await
        .unwrap();
    let shell: ShellProxy = crate::dbus::shell::connect(served.connection()).await.unwrap();
    assert_eq!(shell.save_clipboard().await.unwrap(), 41);
    assert_eq!(shell.get_text("primary").await.unwrap(), (true, "from primary".to_string()));
    assert_eq!(shell.get_text("clipboard").await.unwrap(), (false, "from clipboard".to_string()));
    shell.set_text("x").await.unwrap();
    shell.restore_clipboard(41).await.unwrap();
}
