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
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

const INPUT_DIR: &str = "/dev/input";

/// Name of the uinput device Dictto types through; reading it back would feed our own
/// injected keys into the shortcut tracker.
pub const VIRTUAL_KEYBOARD_NAME: &str = "Dictto virtual keyboard";

#[derive(Debug, thiserror::Error)]
pub enum KeyboardError {
    #[error("no permission to read the keyboards in /dev/input")]
    PermissionDenied,
    #[error("cannot watch /dev/input: {0}")]
    Io(#[from] io::Error),
}

/// What an opened node reports before the set decides to read it.
pub struct Probe<N> {
    pub name: Option<String>,
    pub keyboard: bool,
    pub node: N,
}

/// Identifies one open of a node, so a late failure report cannot drop a reopened device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub name: OsString,
    pub id: u64,
}

pub trait Opener {
    type Node;
    type Device;
    fn open(&mut self, path: &Path) -> io::Result<Probe<Self::Node>>;
    /// Starts reading an accepted node; the reader reports `failure` when it stops on error.
    fn start(&mut self, node: Self::Node, failure: Failure) -> Self::Device;
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
    devices: HashMap<OsString, (u64, O::Device)>,
    next_id: u64,
}

impl<O: Opener> DeviceSet<O> {
    pub fn new(opener: O, dir: impl Into<PathBuf>) -> Self {
        Self { opener, dir: dir.into(), devices: HashMap::new(), next_id: 0 }
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
        let probe = match self.opener.open(&self.dir.join(name)) {
            Ok(probe) => probe,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Created::Denied,
            Err(error) => {
                log::debug!("cannot open {}: {error}", self.dir.join(name).display());
                return Created::Failed;
            }
        };
        if !probe.keyboard || probe.name.as_deref() == Some(VIRTUAL_KEYBOARD_NAME) {
            return Created::Ignored;
        }
        self.next_id += 1;
        let failure = Failure { name: name.to_owned(), id: self.next_id };
        let device = self.opener.start(probe.node, failure);
        self.devices.insert(name.to_owned(), (self.next_id, device));
        Created::Opened
    }

    /// True when the node was an open keyboard.
    pub fn on_removed(&mut self, name: &OsStr) -> bool {
        self.devices.remove(name).is_some()
    }

    /// Drops a reader that stopped on error and tries the node again, since a reader
    /// failure without a DELETE would otherwise keep the node from ever being reopened.
    /// `None` when the report is stale (the node was removed or reopened since).
    pub fn on_reader_failed(&mut self, failure: &Failure) -> Option<Created> {
        match self.devices.get(&failure.name) {
            Some((id, _)) if *id == failure.id => {}
            _ => return None,
        }
        self.devices.remove(&failure.name);
        Some(self.on_created(&failure.name))
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
    let (failures, failed) = mpsc::unbounded_channel();
    spawn_in(Path::new(INPUT_DIR), EvdevOpener { failures }, failed)
}

fn spawn_in<O>(
    dir: &Path,
    opener: O,
    failed: UnboundedReceiver<Failure>,
) -> Result<KeyboardHandle, KeyboardError>
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
    let watcher = async_runtime::spawn(watch(inotify, set, failed));
    Ok(KeyboardHandle { watcher })
}

fn list(dir: &Path) -> io::Result<Vec<OsString>> {
    std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

enum Change {
    Created(OsString),
    // ATTRIB: udev grants access after creating the node, so the first open can fail.
    Changed(OsString),
    Removed(OsString),
    Overflow,
}

// A hotplug failure only disables hotplug: the set must outlive it so the keyboards
// already being read keep working and reader failures are still handled.
async fn watch<O: Opener>(inotify: Inotify, mut set: DeviceSet<O>, mut failed: UnboundedReceiver<Failure>) {
    let mut fd = AsyncFd::new(inotify)
        .map_err(|error| log::error!("keyboard hotplug unavailable: {error}"))
        .ok();
    let mut buffer = [0u8; 4096];
    loop {
        tokio::select! {
            changes = next_changes(fd.as_mut(), &mut buffer) => match changes {
                Ok(changes) => {
                    for change in changes {
                        apply(&mut set, change);
                    }
                }
                Err(error) => {
                    log::error!("keyboard hotplug stopped: {error}");
                    fd = None;
                }
            },
            Some(failure) = failed.recv() => {
                if let Some(result) = set.on_reader_failed(&failure) {
                    log::info!("reopening {}: {result:?}", failure.name.to_string_lossy());
                }
            }
        }
    }
}

async fn next_changes(fd: Option<&mut AsyncFd<Inotify>>, buffer: &mut [u8]) -> io::Result<Vec<Change>> {
    let Some(fd) = fd else {
        return std::future::pending().await;
    };
    loop {
        let mut guard = fd.readable_mut().await?;
        if let Ok(result) = guard.try_io(|fd| {
            let events = fd.get_mut().read_events(buffer)?;
            Ok(events.filter_map(change).collect())
        }) {
            return result;
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
    } else if event.mask.contains(EventMask::ATTRIB) {
        Some(Change::Changed(name))
    } else {
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
        Change::Changed(name) => match set.on_created(&name) {
            Created::Opened => log::info!("keyboard added: {}", name.to_string_lossy()),
            // Still refused once udev has applied permissions: the user lacks access.
            Created::Denied => log::info!(
                "no permission to read {}",
                set.dir.join(&name).display()
            ),
            _ => {}
        },
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

struct EvdevOpener {
    failures: UnboundedSender<Failure>,
}

/// Reading task of one keyboard, cancelled when the device leaves the set.
struct Reader(JoinHandle<()>);

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Opener for EvdevOpener {
    type Node = evdev::Device;
    type Device = Reader;

    fn open(&mut self, path: &Path) -> io::Result<Probe<evdev::Device>> {
        let device = evdev::Device::open(path)?;
        let keyboard = device
            .supported_keys()
            .is_some_and(|keys| keys.contains(evdev::KeyCode::KEY_A));
        Ok(Probe { name: device.name().map(str::to_owned), keyboard, node: device })
    }

    fn start(&mut self, device: evdev::Device, failure: Failure) -> Reader {
        let path = Path::new(INPUT_DIR).join(&failure.name);
        Reader(async_runtime::spawn(read(device, path, failure, self.failures.clone())))
    }
}

async fn read(device: evdev::Device, path: PathBuf, failure: Failure, failures: UnboundedSender<Failure>) {
    // The stream registers with the reactor, so it is made on the runtime, not by the opener.
    let result = match device.into_event_stream() {
        Ok(mut events) => loop {
            match events.next_event().await {
                Ok(event) => {
                    if let Some(down) = keymap::transition(event.event_type().0, event.value()) {
                        // The source is the bare key code, so two keyboards holding the
                        // same key count as one key until either releases it.
                        let code = event.code();
                        let _ = dictation::key_event(code, keymap::from_evdev(code), down);
                    }
                }
                Err(error) => break error,
            }
        },
        Err(error) => error,
    };
    log::info!("stopped reading {}: {result}", path.display());
    // Releases a chord whose key-up this device will never deliver.
    dictation::reset_input();
    let _ = failures.send(failure);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reports every node as a keyboard named after its file, refuses all with EACCES
    /// when `denied`, or names one node like our own virtual keyboard.
    #[derive(Default)]
    struct Fake {
        denied: bool,
        virtual_node: Option<&'static str>,
        opens: usize,
    }

    impl Opener for Fake {
        type Node = PathBuf;
        type Device = (PathBuf, u64);
        fn open(&mut self, path: &Path) -> io::Result<Probe<PathBuf>> {
            if self.denied {
                return Err(io::Error::from_raw_os_error(13));
            }
            self.opens += 1;
            let file = path.file_name().unwrap().to_str().unwrap();
            let name = if Some(file) == self.virtual_node { VIRTUAL_KEYBOARD_NAME } else { file };
            Ok(Probe { name: Some(name.to_owned()), keyboard: true, node: path.to_owned() })
        }
        fn start(&mut self, node: PathBuf, failure: Failure) -> (PathBuf, u64) {
            (node, failure.id)
        }
    }

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn id_of(set: &DeviceSet<Fake>, name: &str) -> Option<u64> {
        set.devices.get(OsStr::new(name)).map(|(id, _)| *id)
    }

    #[test]
    fn new_event_device_is_opened() {
        let mut set = DeviceSet::new(Fake::default(), "/dev/input");
        assert_eq!(set.on_created(OsStr::new("event7")), Created::Opened);
        let (_, (path, _)) = &set.devices[OsStr::new("event7")];
        assert_eq!(path, &PathBuf::from("/dev/input/event7"));
        assert_eq!(set.on_created(OsStr::new("event7")), Created::AlreadyOpen);
        assert!(set.on_removed(OsStr::new("event7")));
        assert!(set.is_empty());
        assert!(!set.on_removed(OsStr::new("event7")));
    }

    #[test]
    fn non_event_nodes_are_ignored() {
        let mut set = DeviceSet::new(Fake::default(), "/dev/input");
        for name in ["mice", "mouse0", "by-id", "event", "eventX"] {
            assert_eq!(set.on_created(OsStr::new(name)), Created::Ignored);
        }
        assert!(set.is_empty());
    }

    #[test]
    fn own_virtual_keyboard_is_not_read() {
        let fake = Fake { virtual_node: Some("event9"), ..Fake::default() };
        let mut set = DeviceSet::new(fake, "/dev/input");
        assert_eq!(set.on_created(OsStr::new("event9")), Created::Ignored);
        assert_eq!(set.on_created(OsStr::new("event3")), Created::Opened);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn failed_reader_is_dropped_and_reopened() {
        let mut set = DeviceSet::new(Fake::default(), "/dev/input");
        set.on_created(OsStr::new("event7"));
        let first = Failure { name: "event7".into(), id: id_of(&set, "event7").unwrap() };
        assert_eq!(set.on_reader_failed(&first), Some(Created::Opened));
        assert_eq!(set.opener.opens, 2);
        let reopened = id_of(&set, "event7").unwrap();
        assert_ne!(reopened, first.id);
        // A late report from the first reader must not drop the reopened device.
        assert_eq!(set.on_reader_failed(&first), None);
        assert_eq!(id_of(&set, "event7"), Some(reopened));
        assert!(set.on_removed(OsStr::new("event7")));
        let current = Failure { name: "event7".into(), id: reopened };
        assert_eq!(set.on_reader_failed(&current), None);
    }

    #[test]
    fn retain_drops_vanished_nodes() {
        let mut set = DeviceSet::new(Fake::default(), "/dev/input");
        set.scan(names(&["event1", "event2"])).unwrap();
        assert!(set.retain(&names(&["event2"])));
        assert!(!set.retain(&names(&["event2"])));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn scan_without_keyboards_is_not_an_error() {
        let mut set = DeviceSet::new(Fake::default(), "/dev/input");
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
        let (_failures, failed) = mpsc::unbounded_channel();
        let result = spawn_in(&dir, Fake { denied: true, ..Fake::default() }, failed);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(result, Err(KeyboardError::PermissionDenied)));
    }
}
