//! Reads the text to speak: the focused accessible's selection through AT-SPI, else the
//! primary selection, else the clipboard, both through the Shell extension. Nothing here
//! writes the clipboard.
//!
//! AT-SPI is called with raw zbus messages instead of the `atspi` crate: four methods on
//! three interfaces do not justify its proxies, event machinery and extra dependencies.
use crate::dbus::shell::ShellProxy;
use dictto_core::host::async_runtime;
use dictto_core::speech::SelectionReader;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use zbus::zvariant::{OwnedObjectPath, Type};

/// Budget for the whole AT-SPI lookup; a hung application must not delay the read.
pub const A11Y_TIMEOUT: Duration = Duration::from_millis(500);
/// Per application, so one unresponsive app does not use the whole budget.
const APP_TIMEOUT: Duration = Duration::from_millis(100);
/// The extension caps its own clipboard reads at 2 s.
const SHELL_TIMEOUT: Duration = Duration::from_millis(2500);
/// Bound on the tree walk for toolkits without the Collection interface (GTK 4).
const MAX_VISITED: usize = 300;
const NOTHING: &str = "Select text or copy it to the clipboard first.";

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COLLECTION: &str = "org.a11y.atspi.Collection";
const TEXT: &str = "org.a11y.atspi.Text";
const STATE_ACTIVE: u32 = 1;
const STATE_FOCUSED: u32 = 12;
const ROLE_PASSWORD_TEXT: u32 = 40;
const MATCH_ALL: i32 = 1;
const SORT_CANONICAL: u32 = 1;

/// Where the text can come from; faked in tests.
pub trait Sources: Send + Sync + 'static {
    fn focused(&self) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// `which` is "primary" or "clipboard".
    fn shell_text(&self, which: &'static str) -> impl Future<Output = Result<Option<String>, String>> + Send;
}

fn usable(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.trim().is_empty())
}

pub async fn pick<S: Sources>(sources: &S) -> Result<String, String> {
    match timeout(A11Y_TIMEOUT, sources.focused()).await {
        Ok(Ok(text)) => {
            if let Some(text) = usable(text) {
                return Ok(text);
            }
        }
        Ok(Err(error)) => log::debug!("AT-SPI selection unavailable: {error}"),
        Err(_) => log::debug!("AT-SPI selection timed out"),
    }
    for which in ["primary", "clipboard"] {
        match timeout(SHELL_TIMEOUT, sources.shell_text(which)).await {
            Ok(Ok(text)) => {
                if let Some(text) = usable(text) {
                    return Ok(text);
                }
            }
            Ok(Err(error)) => log::debug!("cannot read the {which} selection: {error}"),
            Err(_) => log::debug!("reading the {which} selection timed out"),
        }
    }
    Err(NOTHING.into())
}

type ObjectRef = (String, OwnedObjectPath);

fn has_state(states: &[u32], state: u32) -> bool {
    states
        .get((state / 32) as usize)
        .is_some_and(|word| word & (1 << (state % 32)) != 0)
}

async fn call<B, R>(bus: &zbus::Connection, object: &ObjectRef, interface: &str, method: &str, body: &B) -> zbus::Result<R>
where
    B: Serialize + Type,
    R: DeserializeOwned + Type,
{
    let reply = bus
        .call_method(Some(object.0.as_str()), object.1.as_str(), Some(interface), method, body)
        .await?;
    reply.body().deserialize()
}

async fn children(bus: &zbus::Connection, object: &ObjectRef) -> zbus::Result<Vec<ObjectRef>> {
    let children: Vec<ObjectRef> = call(bus, object, ACCESSIBLE, "GetChildren", &()).await?;
    Ok(children.into_iter().filter(|(_, path)| path.as_str() != "/org/a11y/atspi/null").collect())
}

async fn states(bus: &zbus::Connection, object: &ObjectRef) -> zbus::Result<Vec<u32>> {
    call(bus, object, ACCESSIBLE, "GetState", &()).await
}

async fn a11y_bus(session: &zbus::Connection) -> zbus::Result<zbus::Connection> {
    let reply = session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())
        .await?;
    let address: String = reply.body().deserialize()?;
    zbus::connection::Builder::address(address.as_str())?.build().await
}

async fn active_window(bus: &zbus::Connection) -> zbus::Result<Option<ObjectRef>> {
    let root: ObjectRef = (
        "org.a11y.atspi.Registry".into(),
        OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root")?,
    );
    for app in children(bus, &root).await? {
        let Ok(Ok(windows)) = timeout(APP_TIMEOUT, children(bus, &app)).await else {
            continue;
        };
        for window in windows {
            if let Ok(Ok(states)) = timeout(APP_TIMEOUT, states(bus, &window)).await {
                if has_state(&states, STATE_ACTIVE) {
                    return Ok(Some(window));
                }
            }
        }
    }
    Ok(None)
}

async fn focused_in(bus: &zbus::Connection, window: &ObjectRef) -> zbus::Result<Option<ObjectRef>> {
    let focused = [1i32 << STATE_FOCUSED, 0];
    let rule = (
        focused.to_vec(),
        MATCH_ALL,
        HashMap::<String, String>::new(),
        MATCH_ALL,
        Vec::<i32>::new(),
        MATCH_ALL,
        Vec::<String>::new(),
        MATCH_ALL,
        false,
    );
    let matches: zbus::Result<Vec<ObjectRef>> =
        call(bus, window, COLLECTION, "GetMatches", &(rule, SORT_CANONICAL, 1i32, true)).await;
    if let Ok(matches) = matches {
        return Ok(matches.into_iter().next());
    }
    let mut queue = VecDeque::from(children(bus, window).await?);
    let mut visited = 0;
    while let Some(object) = queue.pop_front() {
        visited += 1;
        if visited > MAX_VISITED {
            break;
        }
        // A child that fails to answer is skipped, as `children` skips missing ones.
        let Ok(states) = states(bus, &object).await else {
            continue;
        };
        if has_state(&states, STATE_FOCUSED) {
            return Ok(Some(object));
        }
        queue.extend(children(bus, &object).await.unwrap_or_default());
    }
    Ok(None)
}

async fn selected_text(bus: &zbus::Connection, object: &ObjectRef) -> zbus::Result<Option<String>> {
    let role: u32 = call(bus, object, ACCESSIBLE, "GetRole", &()).await?;
    if role == ROLE_PASSWORD_TEXT {
        return Ok(None);
    }
    // Without the Text interface there is no selection to read here.
    let Ok(count) = call::<_, i32>(bus, object, TEXT, "GetNSelections", &()).await else {
        return Ok(None);
    };
    let mut parts = Vec::new();
    for index in 0..count.clamp(0, 16) {
        let (start, end): (i32, i32) = call(bus, object, TEXT, "GetSelection", &index).await?;
        if start < end {
            parts.push(call::<_, String>(bus, object, TEXT, "GetText", &(start, end)).await?);
        }
    }
    Ok(Some(parts.join("\n")))
}

struct Desktop {
    shell: ShellProxy<'static>,
}

impl Sources for Desktop {
    async fn focused(&self) -> Result<Option<String>, String> {
        async {
            let bus = a11y_bus(self.shell.inner().connection()).await?;
            match active_window(&bus).await? {
                Some(window) => match focused_in(&bus, &window).await? {
                    Some(object) => selected_text(&bus, &object).await,
                    None => Ok(None),
                },
                None => Ok(None),
            }
        }
        .await
        .map_err(|e: zbus::Error| e.to_string())
    }
    async fn shell_text(&self, which: &'static str) -> Result<Option<String>, String> {
        let (found, text) = self.shell.get_text(which).await.map_err(|e| e.to_string())?;
        Ok(found.then_some(text))
    }
}

pub struct AtspiSelection {
    sources: Arc<Desktop>,
}

impl AtspiSelection {
    pub fn new(shell: ShellProxy<'static>) -> Self {
        Self { sources: Arc::new(Desktop { shell }) }
    }
}

impl SelectionReader for AtspiSelection {
    /// Called on a blocking thread by the core; the lookup runs on the core runtime,
    /// where the session connection lives.
    fn read(&self) -> Result<String, String> {
        let sources = self.sources.clone();
        let (reply, answer) = std::sync::mpsc::channel();
        async_runtime::spawn(async move {
            let _ = reply.send(pick(&*sources).await);
        });
        answer
            .recv_timeout(A11Y_TIMEOUT + SHELL_TIMEOUT * 2 + Duration::from_secs(1))
            .map_err(|_| "Reading the selected text timed out.".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        focused: Option<String>,
        hang: bool,
        primary: Option<String>,
        clipboard: Option<String>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl Sources for Fake {
        async fn focused(&self) -> Result<Option<String>, String> {
            self.calls.lock().unwrap().push("focused");
            if self.hang {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Ok(self.focused.clone())
        }
        async fn shell_text(&self, which: &'static str) -> Result<Option<String>, String> {
            self.calls.lock().unwrap().push(which);
            Ok(if which == "primary" { self.primary.clone() } else { self.clipboard.clone() })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn selection_falls_back_to_primary_then_clipboard() {
        let focused = Fake { focused: Some("focused".into()), primary: Some("p".into()), ..Default::default() };
        assert_eq!(pick(&focused).await.unwrap(), "focused");
        assert_eq!(*focused.calls.lock().unwrap(), ["focused"]);

        let primary = Fake { focused: Some("  ".into()), primary: Some("p".into()), clipboard: Some("c".into()), ..Default::default() };
        assert_eq!(pick(&primary).await.unwrap(), "p");
        assert_eq!(*primary.calls.lock().unwrap(), ["focused", "primary"]);

        let clipboard = Fake { hang: true, clipboard: Some("c".into()), ..Default::default() };
        let started = tokio::time::Instant::now();
        assert_eq!(pick(&clipboard).await.unwrap(), "c");
        assert_eq!(started.elapsed(), A11Y_TIMEOUT);
        assert_eq!(*clipboard.calls.lock().unwrap(), ["focused", "primary", "clipboard"]);

        assert_eq!(pick(&Fake::default()).await.unwrap_err(), NOTHING);
    }

    #[test]
    fn state_bits_span_both_words() {
        assert!(has_state(&[1 << STATE_FOCUSED, 0], STATE_FOCUSED));
        assert!(!has_state(&[1 << STATE_ACTIVE, 0], STATE_FOCUSED));
        assert!(has_state(&[0, 1 << 2], 34));
        assert!(!has_state(&[], STATE_ACTIVE));
    }
}
