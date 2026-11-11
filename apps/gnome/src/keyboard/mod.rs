//! Global shortcut source for Wayland: GNOME offers no key hook to clients, so the daemon
//! reads every keyboard under /dev/input and follows hotplug with inotify. Reading needs
//! membership of the `input` group; evdev cannot swallow events, so `key_event`'s verdict
//! is ignored.
pub mod keymap;

use dictto_core::dictation;
use dictto_core::host::async_runtime::{self, JoinHandle};
use inotify::{EventMask, Inotify, WatchMask};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use tokio::io::unix::AsyncFd;

const INPUT_DIR: &str = "/dev/input";

#[derive(Debug, thiserror::Error)]
pub enum KeyboardError {
    #[error("no permission to read the keyboards in /dev/input")]
    PermissionDenied,
    #[error("cannot watch /dev/input: {0}")]
    Io(#[from] io::Error),
}

/// Opens one device node. `Ok(None)` means the node is not a keyboard.
pub trait Opener {
    type Device;
    fn open(&mut self, path: &Path) -> io::Result<Option<Self::Device>>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Created {
    Opened,
    AlreadyOpen,
    Ignored,
    Denied,
    Failed,
}

/// The open keyboards by node name; dropping a device stops reading it.
pub struct DeviceSet<O: Opener> {
    opener: O,
    dir: PathBuf,
    devices: HashMap<OsString, O::Device>,
}

impl<O: Opener> DeviceSet<O> {
    pub fn new(opener: O, dir: impl Into<PathBuf>) -> Self {
        Self { opener, dir: dir.into(), devices: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn on_created(&mut self, name: &OsStr) -> Created {
        if !is_event_node(name) {
            return Created::Ignored;
        }
        if self.devices.contains_key(name) {
            return Created::AlreadyOpen;
        }
        match self.opener.open(&self.dir.join(name)) {
            Ok(Some(device)) => {
                self.devices.insert(name.to_owned(), device);
                Created::Opened
            }
            Ok(None) => Created::Ignored,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Created::Denied,
            Err(error) => {
                log::debug!("cannot open {}: {error}", self.dir.join(name).display());
                Created::Failed
            }
        }
    }

    /// True when the node was an open keyboard.
    pub fn on_removed(&mut self, name: &OsStr) -> bool {
        self.devices.remove(name).is_some()
    }

    /// Opens every keyboard among `names`. Fails only when none is open and access was
    /// refused, since a system with no keyboard yet can still gain one by hotplug.
    pub fn scan<I: IntoIterator<Item = OsString>>(&mut self, names: I) -> Result<(), KeyboardError> {
        let mut denied = false;
        for name in names {
            denied |= self.on_created(&name) == Created::Denied;
        }
        if self.devices.is_empty() && denied {
            return Err(KeyboardError::PermissionDenied);
        }
        Ok(())
    }

    /// Drops the devices whose node is gone; true when any was dropped.
    pub fn retain(&mut self, names: &[OsString]) -> bool {
        let before = self.devices.len();
        self.devices.retain(|name, _| names.contains(name));
        self.devices.len() != before
    }
}

fn is_event_node(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix("event"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Stops the hotplug watcher and every device reader when dropped.
pub struct KeyboardHandle {
    watcher: JoinHandle<()>,
}

impl Drop for KeyboardHandle {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

/// Starts reading the keyboards on the core's runtime, which must be initialized. On error
/// nothing keeps running, so the caller can retry once the user has been granted access.
pub fn spawn() -> Result<KeyboardHandle, KeyboardError> {
    spawn_in(Path::new(INPUT_DIR), EvdevOpener)
}

fn spawn_in<O>(dir: &Path, opener: O) -> Result<KeyboardHandle, KeyboardError>
where
    O: Opener + Send + 'static,
    O::Device: Send,
{
    // Watching before the scan queues any device that appears in between.
    let inotify = Inotify::init()?;
    inotify
        .watches()
        .add(dir, WatchMask::CREATE | WatchMask::ATTRIB | WatchMask::DELETE)?;
    let mut set = DeviceSet::new(opener, dir);
    set.scan(list(dir)?)?;
    log::info!("reading {} keyboard(s)", set.len());
    let watcher = async_runtime::spawn(watch(inotify, set));
    Ok(KeyboardHandle { watcher })
}

fn list(dir: &Path) -> io::Result<Vec<OsString>> {
    std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

enum Change {
    Created(OsString),
    Removed(OsString),
    Overflow,
}

// On a hotplug failure the task parks instead of returning: returning would drop the set
// and stop the keyboards already being read.
async fn watch<O: Opener>(inotify: Inotify, mut set: DeviceSet<O>) {
    let mut fd = match AsyncFd::new(inotify) {
        Ok(fd) => fd,
        Err(error) => {
            log::error!("keyboard hotplug unavailable: {error}");
            return std::future::pending().await;
        }
    };
    let mut buffer = [0u8; 4096];
    loop {
        let mut guard = match fd.readable_mut().await {
            Ok(guard) => guard,
            Err(error) => {
                log::error!("keyboard hotplug stopped: {error}");
                return std::future::pending().await;
            }
        };
        let changes = match guard.try_io(|fd| {
            let events = fd.get_mut().read_events(&mut buffer)?;
            Ok(events.filter_map(change).collect::<Vec<_>>())
        }) {
            Ok(Ok(changes)) => changes,
            Ok(Err(error)) => {
                log::error!("keyboard hotplug stopped: {error}");
                return std::future::pending().await;
            }
            Err(_would_block) => continue,
        };
        for change in changes {
            apply(&mut set, change);
        }
    }
}

fn change(event: inotify::Event<&OsStr>) -> Option<Change> {
    if event.mask.contains(EventMask::Q_OVERFLOW) {
        return Some(Change::Overflow);
    }
    let name = event.name?.to_owned();
    if event.mask.contains(EventMask::DELETE) {
        Some(Change::Removed(name))
    } else {
        // ATTRIB too: udev grants access after creating the node, so the first open can fail.
        Some(Change::Created(name))
    }
}

fn apply<O: Opener>(set: &mut DeviceSet<O>, change: Change) {
    match change {
        Change::Created(name) => {
            if set.on_created(&name) == Created::Opened {
                log::info!("keyboard added: {}", name.to_string_lossy());
            }
        }
        Change::Removed(name) => {
            if set.on_removed(&name) {
                log::info!("keyboard removed: {}", name.to_string_lossy());
                dictation::reset_input();
            }
        }
        Change::Overflow => match list(&set.dir) {
            Ok(names) => {
                if set.retain(&names) {
                    dictation::reset_input();
                }
                let _ = set.scan(names);
            }
            Err(error) => log::warn!("cannot rescan {}: {error}", set.dir.display()),
        },
    }
}

struct EvdevOpener;

/// Reading task of one keyboard, cancelled when the device leaves the set.
struct Reader(JoinHandle<()>);

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Opener for EvdevOpener {
    type Device = Reader;

    fn open(&mut self, path: &Path) -> io::Result<Option<Reader>> {
        let device = evdev::Device::open(path)?;
        let is_keyboard = device
            .supported_keys()
            .is_some_and(|keys| keys.contains(evdev::KeyCode::KEY_A));
        if !is_keyboard {
            return Ok(None);
        }
        Ok(Some(Reader(async_runtime::spawn(read(device, path.to_owned())))))
    }
}

async fn read(device: evdev::Device, path: PathBuf) {
    // The stream registers with the reactor, so it is made on the runtime, not by the opener.
    let mut events = match device.into_event_stream() {
        Ok(events) => events,
        Err(error) => {
            log::warn!("cannot read {}: {error}", path.display());
            return;
        }
    };
    loop {
        match events.next_event().await {
            Ok(event) => {
                if let Some(down) = keymap::transition(event.event_type().0, event.value()) {
                    let code = event.code();
                    let _ = dictation::key_event(code, keymap::from_evdev(code), down);
                }
            }
            Err(error) => {
                log::info!("stopped reading {}: {error}", path.display());
                // Releases a chord whose key-up this device will never deliver.
                dictation::reset_input();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens every node as a keyboard, or refuses them all with EACCES.
    struct Fake {
        denied: bool,
    }

    impl Opener for Fake {
        type Device = PathBuf;
        fn open(&mut self, path: &Path) -> io::Result<Option<PathBuf>> {
            if self.denied {
                return Err(io::Error::from_raw_os_error(13));
            }
            Ok(Some(path.to_owned()))
        }
    }

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn new_event_device_is_opened() {
        let mut set = DeviceSet::new(Fake { denied: false }, "/dev/input");
        assert_eq!(set.on_created(OsStr::new("event7")), Created::Opened);
        assert_eq!(set.devices.get(OsStr::new("event7")), Some(&PathBuf::from("/dev/input/event7")));
        assert_eq!(set.on_created(OsStr::new("event7")), Created::AlreadyOpen);
        assert!(set.on_removed(OsStr::new("event7")));
        assert!(set.is_empty());
        assert!(!set.on_removed(OsStr::new("event7")));
    }

    #[test]
    fn non_event_nodes_are_ignored() {
        let mut set = DeviceSet::new(Fake { denied: false }, "/dev/input");
        for name in ["mice", "mouse0", "by-id", "event", "eventX"] {
            assert_eq!(set.on_created(OsStr::new(name)), Created::Ignored);
        }
        assert!(set.is_empty());
    }

    #[test]
    fn retain_drops_vanished_nodes() {
        let mut set = DeviceSet::new(Fake { denied: false }, "/dev/input");
        set.scan(names(&["event1", "event2"])).unwrap();
        assert!(set.retain(&names(&["event2"])));
        assert!(!set.retain(&names(&["event2"])));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn scan_without_keyboards_is_not_an_error() {
        let mut set = DeviceSet::new(Fake { denied: false }, "/dev/input");
        set.scan(names(&["mice"])).unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn permission_denied_is_reported() {
        let dir = std::env::temp_dir().join(format!("dictto-input-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["event0", "event1", "mice"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let result = spawn_in(&dir, Fake { denied: true });
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(result, Err(KeyboardError::PermissionDenied)));
    }
}
