//! Pastes dictated text: the Shell extension owns the clipboard, a uinput keyboard sends Ctrl+V.
use crate::dbus::shell::ShellProxy;
use crate::keyboard::VIRTUAL_KEYBOARD_NAME;
use async_trait::async_trait;
use dictto_core::platform::{InjectionError, TextInjector};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MISSING_EXTENSION: &str = "Dictto's GNOME Shell extension is not running";
const UINPUT_DENIED: &str = "cannot create a virtual keyboard: grant access to /dev/uinput (see the Dictto packaging udev rule)";

/// Compositors ignore a uinput device until they have enumerated it.
const DEVICE_SETTLE: Duration = Duration::from_millis(150);
/// Short gaps make the chord look like a real keypress to GNOME.
const KEY_GAP: Duration = Duration::from_millis(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Problem {
    MissingExtension,
    UinputDenied,
}

impl Problem {
    pub fn status(self) -> &'static str {
        match self {
            Problem::MissingExtension => MISSING_EXTENSION,
            Problem::UinputDenied => "Dictto cannot paste: no access to /dev/uinput",
        }
    }
}

#[async_trait]
#[allow(clippy::double_must_use)]
pub trait Shell: Send + Sync {
    async fn save_clipboard(&self) -> Result<u32, String>;
    async fn restore_clipboard(&self, token: u32) -> Result<(), String>;
    async fn set_text(&self, text: &str) -> Result<(), String>;
}

#[async_trait]
#[allow(clippy::double_must_use)]
pub trait VirtualKeys: Send + Sync {
    /// Creates the device if needed and lets the compositor register it.
    async fn prepare(&self) -> Result<(), String>;
    async fn ctrl_v(&self) -> Result<(), String>;
}

pub trait ProblemReporter: Send + Sync {
    /// `None` clears the problem; the App1 `Status` becomes "ok".
    fn set_status(&self, problem: Option<Problem>);
    fn notify(&self, problem: Problem);
}

#[async_trait]
impl Shell for ShellProxy<'static> {
    async fn save_clipboard(&self) -> Result<u32, String> {
        ShellProxy::save_clipboard(self).await.map_err(|e| e.to_string())
    }
    async fn restore_clipboard(&self, token: u32) -> Result<(), String> {
        ShellProxy::restore_clipboard(self, token).await.map_err(|e| e.to_string())
    }
    async fn set_text(&self, text: &str) -> Result<(), String> {
        ShellProxy::set_text(self, text).await.map_err(|e| e.to_string())
    }
}

/// The virtual keyboard is created on first use and kept for the whole run.
#[derive(Default)]
pub struct UinputKeys {
    device: tokio::sync::Mutex<Option<VirtualDevice>>,
}

impl UinputKeys {
    fn create() -> std::io::Result<VirtualDevice> {
        let mut keys = AttributeSet::<KeyCode>::new();
        keys.insert(KeyCode::KEY_LEFTCTRL);
        keys.insert(KeyCode::KEY_V);
        VirtualDevice::builder()?.name(VIRTUAL_KEYBOARD_NAME).with_keys(&keys)?.build()
    }
}

fn key_event(key: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, key.code(), value)
}

#[async_trait]
impl VirtualKeys for UinputKeys {
    async fn prepare(&self) -> Result<(), String> {
        let mut slot = self.device.lock().await;
        if slot.is_none() {
            // Device creation is a series of blocking ioctls.
            let device = tokio::task::spawn_blocking(Self::create)
                .await
                .map_err(|e| format!("{UINPUT_DENIED}: {e}"))?
                .map_err(|e| format!("{UINPUT_DENIED}: {e}"))?;
            *slot = Some(device);
            tokio::time::sleep(DEVICE_SETTLE).await;
        }
        Ok(())
    }

    async fn ctrl_v(&self) -> Result<(), String> {
        self.prepare().await?;
        let mut slot = self.device.lock().await;
        let device = slot.as_mut().ok_or_else(|| UINPUT_DENIED.to_string())?;
        // emit() appends one SYN_REPORT per call, so each transition is its own frame.
        let chord = [
            (KeyCode::KEY_LEFTCTRL, 1),
            (KeyCode::KEY_V, 1),
            (KeyCode::KEY_V, 0),
            (KeyCode::KEY_LEFTCTRL, 0),
        ];
        let mut result = Ok(());
        for (key, value) in chord {
            if let Err(e) = device.emit(&[key_event(key, value)]) {
                result = Err(format!("virtual keyboard write failed: {e}"));
                // Keep going so Ctrl is never left stuck down.
                continue;
            }
            tokio::time::sleep(KEY_GAP).await;
        }
        result
    }
}

/// Publishes problems on App1 `Status` and as a desktop notification.
pub struct DesktopReporter {
    set_status: Box<dyn Fn(&str) + Send + Sync>,
}

impl DesktopReporter {
    /// `set_status` must not block; it typically spawns `Served::set_status`.
    pub fn new(set_status: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self { set_status: Box::new(set_status) }
    }
}

impl ProblemReporter for DesktopReporter {
    fn set_status(&self, problem: Option<Problem>) {
        (self.set_status)(problem.map_or("ok", Problem::status));
    }

    fn notify(&self, problem: Problem) {
        dictto_core::host::async_runtime::spawn(async move {
            if let Err(e) = crate::notify::show("Dictto cannot paste", problem.status()).await {
                log::warn!("desktop notification failed: {e}");
            }
        });
    }
}

pub struct GnomeInjector<S, K, R> {
    shell: S,
    keys: K,
    reporter: R,
    notified: Mutex<HashSet<Problem>>,
    flagged: AtomicBool,
}

impl<S: Shell, K: VirtualKeys, R: ProblemReporter> GnomeInjector<S, K, R> {
    pub fn new(shell: S, keys: K, reporter: R) -> Self {
        Self {
            shell,
            keys,
            reporter,
            notified: Mutex::new(HashSet::new()),
            flagged: AtomicBool::new(false),
        }
    }

    fn problem(&self, problem: Problem) {
        self.flagged.store(true, Ordering::Relaxed);
        self.reporter.set_status(Some(problem));
        let first = self.notified.lock().map(|mut set| set.insert(problem)).unwrap_or(false);
        if first {
            self.reporter.notify(problem);
        }
    }

    fn shell_error(&self, error: String) -> InjectionError {
        log::warn!("shell clipboard call failed: {error}");
        self.problem(Problem::MissingExtension);
        InjectionError::ClipboardError(MISSING_EXTENSION.into())
    }

    fn keyboard_error(&self, error: String) -> InjectionError {
        self.problem(Problem::UinputDenied);
        InjectionError::KeyboardError(error)
    }

    async fn paste(&self, text: &str) -> Result<(), InjectionError> {
        self.shell.set_text(text).await.map_err(|e| self.shell_error(e))?;
        self.keys.ctrl_v().await.map_err(|e| self.keyboard_error(e))
    }
}

#[async_trait]
impl<S: Shell, K: VirtualKeys, R: ProblemReporter> TextInjector for GnomeInjector<S, K, R> {
    async fn inject(&self, text: &str, paste_delay_ms: u64) -> Result<Instant, InjectionError> {
        // Device creation and its settle time come first so the text stays on the clipboard briefly.
        self.keys.prepare().await.map_err(|e| self.keyboard_error(e))?;

        // Like Windows, a failed save only means there is nothing to restore.
        let token = match self.shell.save_clipboard().await {
            Ok(token) => Some(token),
            Err(e) => {
                log::warn!("Warning: Failed to save clipboard: {e}");
                None
            }
        };
        let pasted = self.paste(text).await;
        let pasted_at = Instant::now();

        if pasted.is_ok() {
            // Keep the text available until the target application has read it.
            tokio::time::sleep(Duration::from_millis(paste_delay_ms)).await;
        }
        if let Some(token) = token {
            if let Err(e) = self.shell.restore_clipboard(token).await {
                log::warn!("Warning: Failed to restore clipboard: {e}");
            }
        }

        pasted?;
        if self.flagged.swap(false, Ordering::Relaxed) {
            self.reporter.set_status(None);
        }
        Ok(pasted_at)
    }
}

#[cfg(test)]
mod tests;
