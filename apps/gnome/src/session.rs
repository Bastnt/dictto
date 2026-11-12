//! Follows the logind session so that shortcuts stop while it is locked or inactive (a
//! switched-away user would otherwise still trigger dictation through /dev/input), and
//! the microphone route reacts like on Windows lock and disconnect.
use dictto_core::host::async_runtime::{self, JoinHandle};
use dictto_core::Runtime;
use futures_util::StreamExt;
use std::time::Duration;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::OwnedObjectPath;

const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionState {
    pub active: bool,
    pub locked: bool,
}

impl SessionState {
    pub fn suspended(self) -> bool {
        !self.active || self.locked
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Active(bool),
    LockedHint(bool),
    /// logind's Unlock signal: a request GNOME acts on, taken only as a resume hint.
    /// Lock is not followed at all; LockedHint reports when the screen is really locked.
    Unlock,
}

/// Suspension policy. Before logind has ever answered, input stays enabled (a system
/// without logind must still dictate). Once a state was observed it is kept across
/// watch failures until a new subscription reads fresh values.
#[derive(Debug, Default)]
pub struct Watcher {
    state: Option<SessionState>,
}

impl Watcher {
    pub fn suspended(&self) -> bool {
        self.state.is_some_and(SessionState::suspended)
    }

    /// Applies an observation; returns the new suspension only when it flips.
    pub fn observe(&mut self, event: SessionEvent) -> Option<bool> {
        let before = self.suspended();
        let mut state = self.state.unwrap_or(SessionState { active: true, locked: false });
        match event {
            SessionEvent::Active(active) => state.active = active,
            SessionEvent::LockedHint(locked) => state.locked = locked,
            SessionEvent::Unlock => state.locked = false,
        }
        self.state = Some(state);
        let after = self.suspended();
        (after != before).then_some(after)
    }

    /// The watch stopped. Never changes the suspension: unknown at startup means enabled,
    /// and a known state stays until fresh values arrive.
    pub fn lost(&self) -> Option<bool> {
        None
    }
}

/// What a suspension change drives; the runtime in production, a recorder in tests.
pub trait SessionSink: Send + Sync + 'static {
    fn input_suspended(&self, suspended: bool);
    fn session_changed(&self, suspended: bool);
}

impl SessionSink for Runtime {
    fn input_suspended(&self, suspended: bool) {
        Runtime::input_suspended(self, suspended);
    }
    fn session_changed(&self, suspended: bool) {
        Runtime::session_changed(self, suspended);
    }
}

fn notify<S: SessionSink>(sink: &S, change: Option<bool>) {
    if let Some(suspended) = change {
        log::info!("session {}", if suspended { "suspended" } else { "resumed" });
        sink.input_suspended(suspended);
        sink.session_changed(suspended);
    }
}

fn feed<S: SessionSink>(sink: &S, watcher: &mut Watcher, event: SessionEvent) {
    notify(sink, watcher.observe(event));
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath>;
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1")]
trait Session {
    #[zbus(property)]
    fn active(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
    #[zbus(signal)]
    fn unlock(&self) -> zbus::Result<()>;
}

/// Watches the session on the core's runtime for the daemon's lifetime, resubscribing
/// with a backoff whenever the bus or logind fails.
pub fn watch<S: SessionSink>(sink: S) -> JoinHandle<()> {
    async_runtime::spawn(async move {
        let mut watcher = Watcher::default();
        let mut delay = RETRY_MIN;
        loop {
            let mut subscribed = false;
            let error = run(&sink, &mut watcher, &mut subscribed).await;
            notify(&sink, watcher.lost());
            if subscribed {
                delay = RETRY_MIN;
            }
            log::warn!("logind session watch interrupted, retrying in {delay:?}: {error}");
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RETRY_MAX);
        }
    })
}

async fn session_path(connection: &zbus::Connection) -> zbus::Result<OwnedObjectPath> {
    let manager = ManagerProxy::new(connection).await?;
    if let Ok(id) = std::env::var("XDG_SESSION_ID") {
        if let Ok(path) = manager.get_session(&id).await {
            return Ok(path);
        }
    }
    match manager.get_session_by_pid(std::process::id()).await {
        Ok(path) => Ok(path),
        // Started by the systemd user manager the daemon is in no session; "auto" then
        // resolves to the user's graphical session.
        Err(_) => manager.get_session("auto").await,
    }
}

/// Reads a property from logind itself, bypassing the proxy cache.
async fn read(properties: &PropertiesProxy<'_>, name: &str) -> zbus::Result<bool> {
    let interface = InterfaceName::from_static_str_unchecked(SESSION_INTERFACE);
    let value = properties.get(interface, name).await?;
    Ok(bool::try_from(value)?)
}

/// Runs one subscription; only returns on failure.
async fn run<S: SessionSink>(sink: &S, watcher: &mut Watcher, subscribed: &mut bool) -> zbus::Error {
    match subscribe(sink, watcher, subscribed).await {
        Ok(never) => match never {},
        Err(error) => error,
    }
}

async fn subscribe<S: SessionSink>(
    sink: &S,
    watcher: &mut Watcher,
    subscribed: &mut bool,
) -> zbus::Result<std::convert::Infallible> {
    let connection = zbus::Connection::system().await?;
    let path = session_path(&connection).await?;
    log::info!("following logind session {}", path.as_str());
    let session = SessionProxy::builder(&connection).path(path.clone())?.build().await?;
    let properties = PropertiesProxy::builder(&connection)
        .destination("org.freedesktop.login1")?
        .path(path)?
        .build()
        .await?;
    // Subscribed before the reads so no change falls between them.
    let mut active = session.receive_active_changed().await;
    let mut locked = session.receive_locked_hint_changed().await;
    let mut unlock = session.receive_unlock().await?;
    feed(sink, watcher, SessionEvent::Active(read(&properties, "Active").await?));
    feed(sink, watcher, SessionEvent::LockedHint(read(&properties, "LockedHint").await?));
    *subscribed = true;
    loop {
        // A change whose value cannot be decoded is re-read from logind; if that fails
        // too the subscription is restarted with the last state kept.
        let event = tokio::select! {
            Some(change) = active.next() => SessionEvent::Active(match change.get().await {
                Ok(value) => value,
                Err(_) => read(&properties, "Active").await?,
            }),
            Some(change) = locked.next() => SessionEvent::LockedHint(match change.get().await {
                Ok(value) => value,
                Err(_) => read(&properties, "LockedHint").await?,
            }),
            Some(_) = unlock.next() => SessionEvent::Unlock,
            else => return Err(zbus::Error::Failure("logind streams ended".into())),
        };
        feed(sink, watcher, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use SessionEvent::{Active, LockedHint, Unlock};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(&'static str, bool)>>);

    impl SessionSink for Arc<Recorder> {
        fn input_suspended(&self, suspended: bool) {
            self.0.lock().unwrap().push(("input", suspended));
        }
        fn session_changed(&self, suspended: bool) {
            self.0.lock().unwrap().push(("session", suspended));
        }
    }

    fn run_events(watcher: &mut Watcher, events: &[SessionEvent]) -> Vec<(&'static str, bool)> {
        let sink = Arc::new(Recorder::default());
        for event in events {
            feed(&sink, watcher, *event);
        }
        let calls = sink.0.lock().unwrap().clone();
        calls
    }

    const SUSPEND: [(&str, bool); 2] = [("input", true), ("session", true)];
    const RESUME: [(&str, bool); 2] = [("input", false), ("session", false)];

    #[test]
    fn locked_hint_suspends_and_resumes() {
        let calls = run_events(&mut Watcher::default(), &[LockedHint(true), LockedHint(false)]);
        assert_eq!(calls, [SUSPEND, RESUME].concat());
    }

    #[test]
    fn inactive_session_suspends() {
        let calls = run_events(&mut Watcher::default(), &[Active(false), Active(true)]);
        assert_eq!(calls, [SUSPEND, RESUME].concat());
    }

    #[test]
    fn repeated_observations_call_once() {
        let events = [Active(true), LockedHint(false), LockedHint(true), LockedHint(true)];
        assert_eq!(run_events(&mut Watcher::default(), &events), SUSPEND);
    }

    #[test]
    fn resumes_only_when_both_active_and_unlocked() {
        let events = [LockedHint(true), Active(false), LockedHint(false), Active(true)];
        assert_eq!(run_events(&mut Watcher::default(), &events), [SUSPEND, RESUME].concat());
    }

    #[test]
    fn unlock_signal_resumes_a_locked_session() {
        let calls = run_events(&mut Watcher::default(), &[LockedHint(true), Unlock]);
        assert_eq!(calls, [SUSPEND, RESUME].concat());
        assert!(run_events(&mut Watcher::default(), &[Unlock]).is_empty());
    }

    #[test]
    fn error_after_locked_state_keeps_input_suspended() {
        let mut watcher = Watcher::default();
        run_events(&mut watcher, &[Active(true), LockedHint(true)]);
        assert_eq!(watcher.lost(), None);
        assert!(watcher.suspended());
        // The next subscription's fresh reads decide.
        assert_eq!(run_events(&mut watcher, &[Active(true), LockedHint(false)]), RESUME);
    }

    #[test]
    fn without_logind_at_startup_input_is_enabled() {
        let watcher = Watcher::default();
        assert_eq!(watcher.lost(), None);
        assert!(!watcher.suspended());
    }
}
