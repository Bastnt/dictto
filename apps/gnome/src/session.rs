//! Follows the logind session so that shortcuts stop while it is locked or inactive (a
//! switched-away user would otherwise still trigger dictation through /dev/input), and
//! the microphone route reacts like on Windows lock and disconnect.
use dictto_core::host::async_runtime::{self, JoinHandle};
use dictto_core::Runtime;
use futures_util::StreamExt;
use zbus::zvariant::OwnedObjectPath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionState {
    pub active: bool,
    pub locked: bool,
}

impl Default for SessionState {
    // The core starts unsuspended; this matches it until logind says otherwise.
    fn default() -> Self {
        Self { active: true, locked: false }
    }
}

impl SessionState {
    pub fn suspended(self) -> bool {
        !self.active || self.locked
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Active(bool),
    Locked(bool),
}

/// Applies an observation; returns the new suspension only when it flips.
pub fn apply(state: &mut SessionState, event: SessionEvent) -> Option<bool> {
    let before = state.suspended();
    match event {
        SessionEvent::Active(active) => state.active = active,
        SessionEvent::Locked(locked) => state.locked = locked,
    }
    let after = state.suspended();
    (after != before).then_some(after)
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

fn feed<S: SessionSink>(sink: &S, state: &mut SessionState, event: SessionEvent) {
    if let Some(suspended) = apply(state, event) {
        log::info!("session {}", if suspended { "suspended" } else { "resumed" });
        sink.input_suspended(suspended);
        sink.session_changed(suspended);
    }
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
    fn lock(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn unlock(&self) -> zbus::Result<()>;
}

/// Watches the session on the core's runtime until the bus goes away.
pub fn watch<S: SessionSink>(sink: S) -> JoinHandle<()> {
    async_runtime::spawn(async move {
        let mut state = SessionState::default();
        if let Err(error) = run(&sink, &mut state).await {
            log::warn!("logind session watch stopped: {error}");
        }
        // Unobserved, a suspension would never lift; shortcuts that work are the lesser risk.
        feed(&sink, &mut state, SessionEvent::Active(true));
        feed(&sink, &mut state, SessionEvent::Locked(false));
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

async fn run<S: SessionSink>(sink: &S, state: &mut SessionState) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let path = session_path(&connection).await?;
    log::info!("following logind session {}", path.as_str());
    let session = SessionProxy::builder(&connection).path(path)?.build().await?;
    let mut active = session.receive_active_changed().await;
    let mut locked = session.receive_locked_hint_changed().await;
    let mut lock = session.receive_lock().await?;
    let mut unlock = session.receive_unlock().await?;
    feed(sink, state, SessionEvent::Active(session.active().await?));
    feed(sink, state, SessionEvent::Locked(session.locked_hint().await?));
    loop {
        // LockedHint as well as Lock/Unlock: unlocking through the GNOME shield emits no
        // Unlock signal, only the hint going back to false.
        let event = tokio::select! {
            Some(change) = active.next() => SessionEvent::Active(change.get().await?),
            Some(change) = locked.next() => SessionEvent::Locked(change.get().await?),
            Some(_) = lock.next() => SessionEvent::Locked(true),
            Some(_) = unlock.next() => SessionEvent::Locked(false),
            else => return Ok(()),
        };
        feed(sink, state, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(&'static str, bool)>>);

    impl SessionSink for std::sync::Arc<Recorder> {
        fn input_suspended(&self, suspended: bool) {
            self.0.lock().unwrap().push(("input", suspended));
        }
        fn session_changed(&self, suspended: bool) {
            self.0.lock().unwrap().push(("session", suspended));
        }
    }

    fn run_events(events: &[SessionEvent]) -> Vec<(&'static str, bool)> {
        let sink = std::sync::Arc::new(Recorder::default());
        let mut state = SessionState::default();
        for event in events {
            feed(&sink, &mut state, *event);
        }
        let calls = sink.0.lock().unwrap().clone();
        calls
    }

    use SessionEvent::{Active, Locked};

    #[test]
    fn lock_and_unlock_suspend_then_resume() {
        assert_eq!(
            run_events(&[Locked(true), Locked(false)]),
            [("input", true), ("session", true), ("input", false), ("session", false)]
        );
    }

    #[test]
    fn inactive_session_suspends() {
        assert_eq!(
            run_events(&[Active(false), Active(true)]),
            [("input", true), ("session", true), ("input", false), ("session", false)]
        );
    }

    #[test]
    fn repeated_observations_call_once() {
        assert_eq!(
            run_events(&[Active(true), Locked(false), Locked(true), Locked(true)]),
            [("input", true), ("session", true)]
        );
    }

    #[test]
    fn resumes_only_when_both_active_and_unlocked() {
        assert_eq!(
            run_events(&[Locked(true), Active(false), Locked(false), Active(true)]),
            [("input", true), ("session", true), ("input", false), ("session", false)]
        );
    }

    #[test]
    fn apply_reports_flips_only() {
        let mut state = SessionState::default();
        assert_eq!(apply(&mut state, Locked(true)), Some(true));
        assert_eq!(apply(&mut state, Active(false)), None);
        assert_eq!(apply(&mut state, Locked(false)), None);
        assert_eq!(apply(&mut state, Active(true)), Some(false));
    }
}
