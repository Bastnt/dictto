//! Capture-node selection, microphone listing and the device monitor. Selection is pure
//! so that it can be tested without a PipeWire daemon.
use super::{Command, Pipewire};
use dictto_core::audio::{AudioError, DeviceMonitorState, DeviceSelection, MicrophoneInfo};
use dictto_core::events;
use dictto_core::host::{AppHandle, Emitter};
use dictto_core::platform::Devices;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::Duration;

const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// Node name of the virtual microphone Dictto creates for speech routing.
pub const VIRTUAL_MIC_NODE: &str = "dictto_microphone";

/// A PipeWire node as announced by the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub media_class: String,
    /// `device.bus` and `device.form-factor`, when the node carries them.
    pub bus: Option<String>,
    pub form_factor: Option<String>,
}

/// Sources, including other virtual ones such as an EasyEffects or filter-chain mic the
/// user made the default. Sink monitors are not source nodes. Dictto's own virtual mic is
/// excluded by name because capturing it while speech is routed would loop it back.
pub fn is_capture_candidate(node: &NodeInfo) -> bool {
    node.media_class.starts_with("Audio/Source") && node.name != VIRTUAL_MIC_NODE
}

/// Resolves the node a capture targets. A selection that names no available node falls
/// back to the default source, then to any source, as the Windows recorder does.
/// `PlaybackLoopback` has no source node and resolves to `None`.
pub fn select_target(
    selection: &DeviceSelection,
    nodes: &[NodeInfo],
    default: Option<&str>,
) -> Option<NodeInfo> {
    let candidates = || nodes.iter().filter(|node| is_capture_candidate(node));
    let named = |name: &str| candidates().find(|node| node.name == name);
    let wanted = match selection {
        DeviceSelection::PlaybackLoopback => return None,
        DeviceSelection::AutoDetect(id) => id.as_deref().and_then(named),
        // The settings may hold the node name or the label the list showed.
        DeviceSelection::ByName(name) => {
            named(name).or_else(|| candidates().find(|node| node.description == *name))
        }
    };
    wanted
        .or_else(|| default.and_then(named))
        .or_else(|| candidates().next())
        .cloned()
}

pub fn microphones(nodes: &[NodeInfo], default: Option<&str>) -> Vec<MicrophoneInfo> {
    nodes
        .iter()
        .filter(|node| is_capture_candidate(node))
        .map(|node| MicrophoneInfo {
            name: node.description.clone(),
            id: node.name.clone(),
            is_default: default == Some(node.name.as_str()),
            form_factor: form_factor(node).into(),
        })
        .collect()
}

/// Same labels as Windows. The device properties are used when the node carries them;
/// otherwise the node name usually names the bus (`bluez_input.*`, `*usb*`).
fn form_factor(node: &NodeInfo) -> &'static str {
    match node.bus.as_deref() {
        Some("bluetooth") => return "Bluetooth",
        Some("usb") => return "USB",
        _ => {}
    }
    match node.form_factor.as_deref() {
        Some("microphone" | "webcam") => return "Microphone",
        Some("headset" | "handset" | "hands-free") => return "Headset",
        _ => {}
    }
    let name = node.name.to_ascii_lowercase();
    if name.starts_with("bluez") {
        "Bluetooth"
    } else if name.contains("usb") {
        "USB"
    } else {
        "Audio"
    }
}

pub struct PipewireDevices {
    pipewire: Arc<Pipewire>,
}

impl PipewireDevices {
    pub fn new(pipewire: Arc<Pipewire>) -> Self {
        Self { pipewire }
    }
}

impl Devices for PipewireDevices {
    fn list_microphones(&self) -> Result<Vec<MicrophoneInfo>, AudioError> {
        let (reply, result) = mpsc::channel();
        self.pipewire.send(Command::ListMicrophones(reply))?;
        result
            .recv_timeout(LIST_TIMEOUT)
            .map_err(|_| AudioError::DeviceError("PipeWire did not list the microphones.".into()))?
    }
}

/// A device monitor registered with the PipeWire thread.
pub(super) struct Watcher {
    pub(super) state: Arc<DeviceMonitorState>,
    pub(super) app: AppHandle,
}

impl Watcher {
    pub(super) fn track(&self, node: &NodeInfo) {
        *self.state.last_device_id.lock().unwrap() = Some(node.name.clone());
        *self.state.last_device_name.lock().unwrap() = node.description.clone();
    }

    pub(super) fn changed(&self) {
        self.state.devices_changed.store(true, Ordering::Release);
        let _ = self.app.emit(events::AUDIO_DEVICES_CHANGED, ());
    }
}

/// Backs `Services::spawn_device_monitor`: follows the default source and the source
/// list like the Windows monitor. Sending on, or dropping, the returned sender stops it.
pub fn spawn_device_monitor(
    pipewire: &Arc<Pipewire>,
    state: Arc<DeviceMonitorState>,
    app: AppHandle,
) -> mpsc::Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    let id = pipewire.next_id();
    if let Err(error) = pipewire.send(Command::Watch { id, state, app }) {
        log::error!("[DeviceMonitor] {error}");
        return stop;
    }
    let pipewire = pipewire.clone();
    let spawned = std::thread::Builder::new()
        .name("device-monitor".into())
        .spawn(move || {
            let _ = stopped.recv();
            let _ = pipewire.send(Command::Unwatch(id));
        });
    if let Err(error) = spawned {
        log::error!("[DeviceMonitor] Cannot start: {error}");
    }
    stop
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, name: &str, description: &str, class: &str) -> NodeInfo {
        NodeInfo {
            id,
            name: name.into(),
            description: description.into(),
            media_class: class.into(),
            bus: None,
            form_factor: None,
        }
    }

    const EFFECTS: &str = "easyeffects_source";

    fn graph() -> Vec<NodeInfo> {
        vec![
            node(
                30,
                "alsa_output.pci.analog-stereo",
                "Speakers",
                "Audio/Sink",
            ),
            node(
                31,
                "alsa_input.pci.analog-stereo",
                "Built-in Microphone",
                "Audio/Source",
            ),
            node(
                32,
                "alsa_input.usb-Blue_Yeti.analog-stereo",
                "Yeti Stereo Microphone",
                "Audio/Source",
            ),
            node(
                33,
                VIRTUAL_MIC_NODE,
                "Dictto Microphone",
                "Audio/Source/Virtual",
            ),
            node(35, EFFECTS, "Easy Effects Source", "Audio/Source/Virtual"),
        ]
    }

    #[test]
    fn default_resolves_to_concrete_node() {
        let nodes = graph();
        let default = Some("alsa_input.usb-Blue_Yeti.analog-stereo");
        let target = select_target(&DeviceSelection::AutoDetect(None), &nodes, default).unwrap();
        assert_eq!(target.id, 32);
        let monitored = DeviceSelection::AutoDetect(Some("alsa_input.pci.analog-stereo".into()));
        assert_eq!(select_target(&monitored, &nodes, default).unwrap().id, 31);
        let stale = DeviceSelection::AutoDetect(Some("bluez_input.gone".into()));
        assert_eq!(select_target(&stale, &nodes, default).unwrap().id, 32);
        assert!(select_target(&DeviceSelection::PlaybackLoopback, &nodes, default).is_none());
    }

    #[test]
    fn named_device_found_by_node_name() {
        let nodes = graph();
        let default = Some("alsa_input.pci.analog-stereo");
        let by_node = DeviceSelection::ByName("alsa_input.usb-Blue_Yeti.analog-stereo".into());
        assert_eq!(select_target(&by_node, &nodes, default).unwrap().id, 32);
        // Settings store the name shown in the list, which is the description.
        let by_label = DeviceSelection::ByName("Yeti Stereo Microphone".into());
        assert_eq!(select_target(&by_label, &nodes, default).unwrap().id, 32);
    }

    #[test]
    fn missing_named_device_falls_back_to_default() {
        let nodes = graph();
        let missing = DeviceSelection::ByName("Unplugged Headset".into());
        let target = select_target(&missing, &nodes, Some("alsa_input.pci.analog-stereo")).unwrap();
        assert_eq!(target.id, 31);
        // Without a usable default, any source is better than failing, like Windows.
        let target = select_target(&missing, &nodes, None).unwrap();
        assert_eq!(target.id, 31);
        assert!(select_target(&missing, &nodes[..1], None).is_none());
    }

    #[test]
    fn virtual_mic_is_never_a_capture_target() {
        let mut nodes = graph();
        // Another module could register it with a plain source class.
        nodes.push(node(
            34,
            VIRTUAL_MIC_NODE,
            "Dictto Microphone",
            "Audio/Source",
        ));
        let default = Some(VIRTUAL_MIC_NODE);
        for selection in [
            DeviceSelection::AutoDetect(None),
            DeviceSelection::AutoDetect(Some(VIRTUAL_MIC_NODE.into())),
            DeviceSelection::ByName(VIRTUAL_MIC_NODE.into()),
            DeviceSelection::ByName("Dictto Microphone".into()),
        ] {
            let target = select_target(&selection, &nodes, default).unwrap();
            assert_ne!(target.name, VIRTUAL_MIC_NODE, "{selection:?}");
        }
        let virtual_only: Vec<_> = nodes
            .iter()
            .filter(|n| n.name == VIRTUAL_MIC_NODE)
            .cloned()
            .collect();
        assert!(
            select_target(&DeviceSelection::AutoDetect(None), &virtual_only, default).is_none()
        );

        let listed = microphones(&nodes, default);
        assert_eq!(listed.len(), 3);
        assert!(listed
            .iter()
            .all(|m| m.id != VIRTUAL_MIC_NODE && !m.is_default));
    }

    #[test]
    fn other_virtual_sources_are_listed_and_selectable() {
        let nodes = graph();
        let target = select_target(&DeviceSelection::AutoDetect(None), &nodes, Some(EFFECTS));
        assert_eq!(target.unwrap().id, 35);
        let by_label = DeviceSelection::ByName("Easy Effects Source".into());
        assert_eq!(select_target(&by_label, &nodes, None).unwrap().id, 35);
        let listed = microphones(&nodes, Some(EFFECTS));
        assert!(listed.iter().any(|m| m.id == EFFECTS && m.is_default));
    }

    #[test]
    fn listing_uses_node_name_as_id_and_description_as_name() {
        let listed = microphones(&graph(), Some("alsa_input.pci.analog-stereo"));
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, "alsa_input.pci.analog-stereo");
        assert_eq!(listed[0].name, "Built-in Microphone");
        assert!(listed[0].is_default);
        assert_eq!(listed[1].form_factor, "USB");
        assert!(!listed[1].is_default);
        assert_eq!(listed[2].id, EFFECTS);
    }

    #[test]
    fn device_properties_win_over_the_name() {
        let mut mic = node(
            40,
            "alsa_input.pci.analog-stereo",
            "Headset",
            "Audio/Source",
        );
        assert_eq!(form_factor(&mic), "Audio");
        mic.form_factor = Some("headset".into());
        assert_eq!(form_factor(&mic), "Headset");
        mic.bus = Some("usb".into());
        assert_eq!(form_factor(&mic), "USB");
        let bluez = node(41, "bluez_input.00_11", "Buds", "Audio/Source");
        assert_eq!(form_factor(&bluez), "Bluetooth");
    }
}
