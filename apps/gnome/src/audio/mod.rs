//! PipeWire audio. One main-loop thread owns every PipeWire object for the process; the
//! recorder, player, sounds, device monitor and virtual microphone reach it through a
//! `pipewire::channel`.
pub mod devices;
pub mod mic;
pub mod player;
pub mod recorder;
pub mod sounds;

pub use devices::{spawn_device_monitor, PipewireDevices};
pub use player::{Pcm, PipewirePlayer, Playing};
pub use recorder::PipewireRecorder;
pub use sounds::PipewireSounds;

use devices::{NodeInfo, Watcher};
use dictto_core::audio::{AudioError, DeviceMonitorState, DeviceSelection, MicrophoneInfo};
use dictto_core::host::AppHandle;
use pipewire as pw;
use pw::spa::utils::result::AsyncSeq;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// How often a device monitor retries while the PipeWire daemon is unreachable.
const RETRY_DELAY: Duration = Duration::from_secs(2);
const DEFAULT_SOURCE_KEY: &str = "default.audio.source";
const DEFAULT_SINK_KEY: &str = "default.audio.sink";

enum Command {
    Capture {
        recorder: u64,
        selection: DeviceSelection,
        app: Option<AppHandle>,
        reply: mpsc::Sender<Result<(), AudioError>>,
    },
    StopCapture {
        recorder: u64,
        reply: mpsc::Sender<Result<Vec<i16>, AudioError>>,
    },
    CancelCapture(u64),
    ListMicrophones(mpsc::Sender<Result<Vec<MicrophoneInfo>, AudioError>>),
    Play {
        id: u64,
        request: player::Request,
    },
    StopPlayback(u64),
    PlaybackEnded(u64),
    Watch {
        id: u64,
        state: Arc<DeviceMonitorState>,
        app: AppHandle,
    },
    Unwatch(u64),
    /// A property of the `default` metadata object, as its raw JSON value.
    MetadataGet {
        key: &'static str,
        reply: mpsc::Sender<Result<Option<String>, String>>,
    },
    MetadataSet {
        key: &'static str,
        value: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
    /// Answered once the virtual microphone node is registered.
    MicCreate(mpsc::Sender<Result<(), String>>),
    MicDestroy,
    /// Number of links from the virtual microphone to a consumer.
    MicLinks(mpsc::Sender<Result<usize, String>>),
    MicPlay {
        samples: Vec<i16>,
        done: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    MicStop,
    /// Carries the serial of the connection that failed, so a late report cannot tear
    /// down a newer connection.
    Disconnected(u64),
}

impl Command {
    /// Commands that read the node list or open streams need a synced connection.
    fn needs_graph(&self) -> bool {
        matches!(
            self,
            Command::Capture { .. }
                | Command::ListMicrophones(_)
                | Command::Play { .. }
                | Command::MetadataGet { .. }
                | Command::MetadataSet { .. }
                | Command::MicCreate(_)
                | Command::MicLinks(_)
        )
    }

    fn fail(self, message: &str) {
        match self {
            Command::MetadataGet { reply, .. } => {
                let _ = reply.send(Err(message.into()));
            }
            Command::MetadataSet { reply, .. } | Command::MicCreate(reply) => {
                let _ = reply.send(Err(message.into()));
            }
            Command::MicLinks(reply) => {
                let _ = reply.send(Err(message.into()));
            }
            Command::Capture { reply, .. } => {
                let _ = reply.send(Err(AudioError::DeviceError(message.into())));
            }
            Command::ListMicrophones(reply) => {
                let _ = reply.send(Err(AudioError::DeviceError(message.into())));
            }
            Command::Play { request, .. } => request.fail(message),
            _ => {}
        }
    }
}

/// Handle to the PipeWire thread.
pub struct Pipewire {
    commands: pw::channel::Sender<Command>,
    ids: AtomicU64,
}

impl Pipewire {
    /// The process-wide instance, started on first use. Creating it needs libpipewire
    /// but no running daemon: the connection is opened by the first command.
    pub fn shared() -> Result<Arc<Pipewire>, AudioError> {
        static SHARED: Mutex<Option<Arc<Pipewire>>> = Mutex::new(None);
        let mut shared = SHARED.lock().unwrap();
        if let Some(pipewire) = shared.as_ref() {
            return Ok(pipewire.clone());
        }
        let pipewire = Arc::new(Self::spawn().map_err(AudioError::DeviceError)?);
        *shared = Some(pipewire.clone());
        Ok(pipewire)
    }

    fn spawn() -> Result<Pipewire, String> {
        let (commands, receiver) = pw::channel::channel();
        let (ready, started) = mpsc::channel();
        let own = commands.clone();
        std::thread::Builder::new()
            .name("pipewire".into())
            .spawn(move || {
                if let Err(error) = run(receiver, own, &ready) {
                    let _ = ready.send(Err(error));
                }
            })
            .map_err(|e| format!("Cannot start the PipeWire thread: {e}"))?;
        started
            .recv()
            .map_err(|_| "The PipeWire thread exited during startup.".to_string())??;
        Ok(Pipewire {
            commands,
            ids: AtomicU64::new(1),
        })
    }

    fn next_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, command: Command) -> Result<(), AudioError> {
        self.commands
            .send(command)
            .map_err(|_| AudioError::DeviceError("The PipeWire thread stopped.".into()))
    }
}

fn run(
    receiver: pw::channel::Receiver<Command>,
    commands: pw::channel::Sender<Command>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    // The loop lives as long as the process; leaking it lets the timers, which borrow
    // the loop, be stored with the graph and streams that arm them.
    let main_loop: &'static pw::main_loop::MainLoopRc = Box::leak(Box::new(
        pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("PipeWire main loop: {e}"))?,
    ));
    let context = pw::context::ContextRc::new(main_loop, None)
        .map_err(|e| format!("PipeWire context: {e}"))?;
    let graph = Rc::new(RefCell::new(Graph::new(
        main_loop.loop_(),
        context,
        commands,
    )));
    let weak = Rc::downgrade(&graph);
    let retry = main_loop.loop_().add_timer(move |_| {
        if let Some(graph) = weak.upgrade() {
            Graph::retry(&graph);
        }
    });
    graph.borrow_mut().retry = Some(retry);
    let weak = Rc::downgrade(&graph);
    let _receiver = receiver.attach(main_loop.loop_(), move |command| {
        if let Some(graph) = weak.upgrade() {
            Graph::handle(&graph, command);
        }
    });
    let _ = ready.send(Ok(()));
    main_loop.run();
    Ok(())
}

/// Listeners are declared before the proxies they hook so that they unregister first.
struct Connection {
    metadata: Option<DefaultMetadata>,
    _registry_listener: pw::registry::Listener,
    registry: pw::registry::RegistryRc,
    _core_listener: pw::core::Listener,
    core: pw::core::CoreRc,
}

struct DefaultMetadata {
    global: u32,
    _listener: pw::metadata::MetadataListener,
    proxy: pw::metadata::Metadata,
}

struct Graph {
    loop_: &'static pw::loop_::Loop,
    context: pw::context::ContextRc,
    /// Lets callbacks defer work that must not run inside the callback itself.
    commands: pw::channel::Sender<Command>,
    retry: Option<pw::loop_::TimerSource<'static>>,
    connection: Option<Connection>,
    /// Serial of the current or last connection.
    serial: u64,
    /// The last sync issued; commands that read the graph wait until it is done.
    pending_sync: Option<AsyncSeq>,
    /// False until the first sync after connecting, so the initial enumeration is not
    /// reported as device changes.
    synced: bool,
    /// Set by a disconnect: the next sync reports a device change.
    stale: bool,
    waiting: Vec<Command>,
    nodes: HashMap<u32, NodeInfo>,
    default_source: Option<String>,
    default_sink: Option<String>,
    sink_generation: u64,
    /// Subject-0 properties of the `default` metadata, raw JSON values by key.
    metadata_values: HashMap<String, String>,
    /// Output node of every link, by link id.
    links: HashMap<u32, u32>,
    mic: Option<mic::Source>,
    mic_waiters: Vec<mpsc::Sender<Result<(), String>>>,
    captures: HashMap<u64, recorder::Capture>,
    playbacks: HashMap<u64, player::Playback>,
    watchers: HashMap<u64, Watcher>,
}

impl Graph {
    fn new(
        loop_: &'static pw::loop_::Loop,
        context: pw::context::ContextRc,
        commands: pw::channel::Sender<Command>,
    ) -> Self {
        Self {
            loop_,
            context,
            commands,
            retry: None,
            connection: None,
            serial: 0,
            pending_sync: None,
            synced: false,
            stale: false,
            waiting: Vec::new(),
            nodes: HashMap::new(),
            default_source: None,
            default_sink: None,
            sink_generation: 0,
            metadata_values: HashMap::new(),
            links: HashMap::new(),
            mic: None,
            mic_waiters: Vec::new(),
            captures: HashMap::new(),
            playbacks: HashMap::new(),
            watchers: HashMap::new(),
        }
    }

    fn handle(this: &Rc<RefCell<Graph>>, command: Command) {
        if let Command::Disconnected(serial) = command {
            // Dropped after the borrow ends: tearing down the core may call back into us.
            let connection = this.borrow_mut().disconnect(serial);
            drop(connection);
            return;
        }
        let mut graph = this.borrow_mut();
        if let Command::Watch { id, state, app } = command {
            graph.watchers.insert(id, Watcher { state, app });
            match graph.connect(this) {
                // Registered even while PipeWire is down: the retry timer connects later
                // and that sync reports the devices.
                Err(error) => {
                    log::warn!("[pipewire] {error}");
                    graph.arm_retry();
                }
                // Read the current default now, as the Windows monitor does at start.
                Ok(()) if graph.pending_sync.is_none() => graph.track_default(false),
                Ok(()) => {}
            }
            return;
        }
        if command.needs_graph() {
            if let Err(error) = graph.connect(this) {
                command.fail(&error.to_string());
                return;
            }
            if graph.pending_sync.is_some() {
                graph.waiting.push(command);
                return;
            }
        }
        graph.execute(command);
    }

    fn execute(&mut self, command: Command) {
        match command {
            Command::Capture {
                recorder,
                selection,
                app,
                reply,
            } => {
                let _ = reply.send(self.start_capture(recorder, selection, app));
            }
            Command::StopCapture { recorder, reply } => {
                let samples = match self.captures.remove(&recorder) {
                    Some(capture) => capture.finish(self.sink_generation),
                    None => Ok(Vec::new()),
                };
                let _ = reply.send(samples);
            }
            Command::CancelCapture(recorder) => {
                self.waiting.retain(
                    |c| !matches!(c, Command::Capture { recorder: r, .. } if *r == recorder),
                );
                self.captures.remove(&recorder);
            }
            Command::ListMicrophones(reply) => {
                let nodes = self.sorted_nodes();
                let _ = reply.send(Ok(devices::microphones(
                    &nodes,
                    self.default_source.as_deref(),
                )));
            }
            Command::Play { id, request } => {
                let Some(connection) = self.connection.as_ref() else {
                    return request.fail("PipeWire is not connected.");
                };
                let commands = self.commands.clone();
                match player::Playback::open(&connection.core, self.loop_, id, request, commands) {
                    Ok(playback) => {
                        self.playbacks.insert(id, playback);
                    }
                    Err(error) => log::warn!("[pipewire] {error}"),
                }
            }
            Command::StopPlayback(id) | Command::PlaybackEnded(id) => {
                self.playbacks.remove(&id);
            }
            Command::Unwatch(id) => {
                self.watchers.remove(&id);
            }
            Command::MetadataGet { key, reply } => {
                let _ = reply.send(Ok(self.metadata_values.get(key).cloned()));
            }
            Command::MetadataSet { key, value, reply } => {
                let metadata = self.connection.as_ref().and_then(|c| c.metadata.as_ref());
                let result = match metadata {
                    Some(metadata) => {
                        // The type wpctl and pactl write, so other tools read it back.
                        metadata.proxy.set_property(
                            pw::core::PW_ID_CORE,
                            key,
                            Some("Spa:String:JSON"),
                            Some(&value),
                        );
                        Ok(())
                    }
                    None => Err("PipeWire has no default devices; is WirePlumber running?".into()),
                };
                let _ = reply.send(result);
            }
            Command::MicCreate(reply) => self.create_mic(reply),
            Command::MicDestroy => {
                self.mic = None;
                for waiter in self.mic_waiters.drain(..) {
                    let _ = waiter.send(Err("The Dictto Microphone was turned off.".into()));
                }
            }
            Command::MicLinks(reply) => {
                let registered = self.mic.as_ref().and_then(|mic| mic.registered);
                let result = match registered {
                    Some(node) => Ok(self.links.values().filter(|output| **output == node).count()),
                    None => Err(mic::MISSING.into()),
                };
                let _ = reply.send(result);
            }
            Command::MicPlay { samples, done } => match self.mic.as_ref() {
                Some(mic) => mic.play(samples, done),
                None => {
                    let _ = done.send(Err(mic::MISSING.into()));
                }
            },
            Command::MicStop => {
                if let Some(mic) = self.mic.as_ref() {
                    mic.stop();
                }
            }
            Command::Watch { .. } | Command::Disconnected(_) => {}
        }
    }

    fn start_capture(
        &mut self,
        recorder: u64,
        selection: DeviceSelection,
        app: Option<AppHandle>,
    ) -> Result<(), AudioError> {
        self.captures.remove(&recorder);
        let core = match self.connection.as_ref() {
            Some(connection) => connection.core.clone(),
            None => return Err(AudioError::DeviceError("PipeWire is not connected.".into())),
        };
        let capture = if let DeviceSelection::PlaybackLoopback = selection {
            recorder::Capture::open(&core, None, Some(self.sink_generation), app)?
        } else {
            let nodes = self.sorted_nodes();
            let target = devices::select_target(&selection, &nodes, self.default_source.as_deref())
                .ok_or(AudioError::NoDevice)?;
            log::info!(
                "[pipewire] Capturing {:?} from {} ({})",
                selection,
                target.name,
                target.description
            );
            recorder::Capture::open(&core, Some(target), None, app)?
        };
        self.captures.insert(recorder, capture);
        Ok(())
    }

    fn create_mic(&mut self, reply: mpsc::Sender<Result<(), String>>) {
        if self.mic.as_ref().is_some_and(|mic| mic.registered.is_some()) {
            let _ = reply.send(Ok(()));
            return;
        }
        if self.mic.is_none() {
            let Some(connection) = self.connection.as_ref() else {
                let _ = reply.send(Err("PipeWire is not connected.".into()));
                return;
            };
            match mic::Source::open(&connection.core) {
                Ok(source) => self.mic = Some(source),
                Err(error) => {
                    let _ = reply.send(Err(error));
                    return;
                }
            }
        }
        self.mic_waiters.push(reply);
    }

    fn sorted_nodes(&self) -> Vec<NodeInfo> {
        let mut nodes: Vec<NodeInfo> = self.nodes.values().cloned().collect();
        nodes.sort_by_key(|node| node.id);
        nodes
    }

    fn connect(&mut self, this: &Rc<RefCell<Graph>>) -> Result<(), AudioError> {
        if self.connection.is_some() {
            return Ok(());
        }
        let unavailable =
            |e: pw::Error| AudioError::DeviceError(format!("PipeWire is unavailable: {e}"));
        let core = self.context.connect_rc(None).map_err(unavailable)?;
        let registry = core.get_registry_rc().map_err(unavailable)?;
        self.serial += 1;
        let serial = self.serial;
        let weak = Rc::downgrade(this);
        let commands = self.commands.clone();
        let core_listener = core
            .add_listener_local()
            .done({
                let weak = weak.clone();
                move |id, seq| {
                    if id == pw::core::PW_ID_CORE {
                        if let Some(graph) = weak.upgrade() {
                            Graph::synced(&graph, seq);
                        }
                    }
                }
            })
            .error(move |id, _, res, message| {
                if id == pw::core::PW_ID_CORE {
                    log::warn!("[pipewire] Connection lost ({res}): {message}");
                    let _ = commands.send(Command::Disconnected(serial));
                } else {
                    log::debug!("[pipewire] Object {id} error ({res}): {message}");
                }
            })
            .register();
        let registry_listener = registry
            .add_listener_local()
            .global({
                let weak = weak.clone();
                move |global| {
                    if let Some(graph) = weak.upgrade() {
                        Graph::global_added(&graph, global);
                    }
                }
            })
            .global_remove(move |id| {
                if let Some(graph) = weak.upgrade() {
                    graph.borrow_mut().global_removed(id);
                }
            })
            .register();
        self.pending_sync = Some(core.sync(0).map_err(unavailable)?);
        self.synced = false;
        self.connection = Some(Connection {
            metadata: None,
            _registry_listener: registry_listener,
            registry,
            _core_listener: core_listener,
            core,
        });
        log::info!("[pipewire] Connected");
        Ok(())
    }

    fn synced(this: &Rc<RefCell<Graph>>, seq: AsyncSeq) {
        let waiting = {
            let mut graph = this.borrow_mut();
            if graph.pending_sync != Some(seq) {
                return;
            }
            graph.pending_sync = None;
            graph.synced = true;
            let stale = std::mem::take(&mut graph.stale);
            graph.track_default(stale);
            std::mem::take(&mut graph.waiting)
        };
        for command in waiting {
            Graph::handle(this, command);
        }
    }

    fn global_added(
        this: &Rc<RefCell<Graph>>,
        global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    ) {
        let mut graph = this.borrow_mut();
        let Some(props) = global.props else { return };
        match global.type_ {
            pw::types::ObjectType::Node => {
                let media_class = props.get("media.class").unwrap_or_default();
                let Some(name) = props.get("node.name") else {
                    return;
                };
                if !media_class.starts_with("Audio/Source") {
                    return;
                }
                let node = NodeInfo {
                    id: global.id,
                    name: name.into(),
                    description: props
                        .get("node.description")
                        .or_else(|| props.get("node.nick"))
                        .unwrap_or(name)
                        .into(),
                    media_class: media_class.into(),
                    bus: props.get("device.bus").map(str::to_owned),
                    form_factor: props.get("device.form-factor").map(str::to_owned),
                };
                let candidate = devices::is_capture_candidate(&node);
                if node.name == devices::VIRTUAL_MIC_NODE {
                    if let Some(mic) = graph.mic.as_mut() {
                        mic.registered = Some(global.id);
                        for waiter in graph.mic_waiters.drain(..) {
                            let _ = waiter.send(Ok(()));
                        }
                    }
                }
                graph.nodes.insert(global.id, node);
                if candidate && graph.synced {
                    graph.track_default(true);
                }
            }
            pw::types::ObjectType::Metadata => {
                if props.get("metadata.name") != Some("default") {
                    return;
                }
                let Some(connection) = graph.connection.as_ref() else {
                    return;
                };
                let proxy: pw::metadata::Metadata = match connection.registry.bind(global) {
                    Ok(proxy) => proxy,
                    Err(e) => {
                        log::warn!("[pipewire] Cannot bind the default metadata: {e}");
                        return;
                    }
                };
                let weak = Rc::downgrade(this);
                let listener = proxy
                    .add_listener_local()
                    .property(move |subject, key, _, value| {
                        if let Some(graph) = weak.upgrade() {
                            graph.borrow_mut().default_changed(subject, key, value);
                        }
                        0
                    })
                    .register();
                // Its properties arrive after another round trip; wait for them.
                let sync = connection.core.sync(0);
                if let Some(connection) = graph.connection.as_mut() {
                    connection.metadata = Some(DefaultMetadata {
                        global: global.id,
                        _listener: listener,
                        proxy,
                    });
                }
                if let Ok(seq) = sync {
                    graph.pending_sync = Some(seq);
                }
            }
            pw::types::ObjectType::Link => {
                if let Some(output) = props.get("link.output.node").and_then(|id| id.parse().ok()) {
                    graph.links.insert(global.id, output);
                }
            }
            _ => {}
        }
    }

    fn global_removed(&mut self, id: u32) {
        if let Some(connection) = self.connection.as_mut() {
            if connection.metadata.as_ref().is_some_and(|m| m.global == id) {
                connection.metadata = None;
                self.metadata_values.clear();
                return;
            }
        }
        if self.links.remove(&id).is_some() {
            return;
        }
        if let Some(mic) = self.mic.as_mut().filter(|mic| mic.registered == Some(id)) {
            mic.registered = None;
        }
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        for capture in self.captures.values() {
            capture.node_removed(&node.name);
        }
        if devices::is_capture_candidate(&node) && self.synced {
            self.track_default(true);
        }
    }

    fn default_changed(&mut self, subject: u32, key: Option<&str>, value: Option<&str>) {
        if subject != pw::core::PW_ID_CORE {
            return;
        }
        match (key, value) {
            (None, _) => self.metadata_values.clear(),
            (Some(key), Some(value)) => {
                self.metadata_values.insert(key.into(), value.into());
            }
            (Some(key), None) => {
                self.metadata_values.remove(key);
            }
        }
        let name = value.and_then(default_name);
        match key {
            None => {
                self.set_default_source(None);
                self.set_default_sink(None);
            }
            Some(DEFAULT_SOURCE_KEY) => self.set_default_source(name),
            Some(DEFAULT_SINK_KEY) => self.set_default_sink(name),
            Some(_) => {}
        }
    }

    fn set_default_source(&mut self, name: Option<String>) {
        if self.default_source != name {
            self.default_source = name;
            if self.synced {
                self.track_default(true);
            }
        }
    }

    fn set_default_sink(&mut self, name: Option<String>) {
        if self.default_sink != name {
            self.default_sink = name;
            self.sink_generation += 1;
        }
    }

    /// Points the monitors at the default source when it is a real microphone, so that
    /// auto-detect keeps the last real one while the virtual mic is the default.
    fn track_default(&self, changed: bool) {
        let default = self.default_source.as_deref().and_then(|name| {
            self.nodes
                .values()
                .find(|node| node.name == name && devices::is_capture_candidate(node))
        });
        for watcher in self.watchers.values() {
            if let Some(node) = default {
                watcher.track(node);
            }
            if changed {
                watcher.changed();
            }
        }
    }

    fn disconnect(&mut self, serial: u64) -> Option<Connection> {
        if serial != self.serial {
            return None;
        }
        let connection = self.connection.take()?;
        log::warn!("[pipewire] Disconnected");
        for capture in self.captures.values_mut() {
            capture.fail(AudioError::DeviceError("PipeWire disconnected.".into()));
        }
        self.playbacks.clear();
        // The stream belonged to the lost core; the next MicCreate builds a new one.
        self.mic = None;
        for waiter in self.mic_waiters.drain(..) {
            let _ = waiter.send(Err("PipeWire disconnected.".into()));
        }
        self.links.clear();
        self.metadata_values.clear();
        self.nodes.clear();
        self.default_source = None;
        self.set_default_sink(None);
        self.pending_sync = None;
        self.synced = false;
        self.stale = true;
        for command in std::mem::take(&mut self.waiting) {
            command.fail("PipeWire disconnected.");
        }
        // Monitors are told once, by the sync after reconnecting, with the final state.
        self.arm_retry();
        Some(connection)
    }

    fn arm_retry(&self) {
        if self.watchers.is_empty() {
            return;
        }
        if let Some(retry) = self.retry.as_ref() {
            let _ = retry.update_timer(Some(RETRY_DELAY), None);
        }
    }

    fn retry(this: &Rc<RefCell<Graph>>) {
        let mut graph = this.borrow_mut();
        if graph.connection.is_some() || graph.watchers.is_empty() {
            return;
        }
        if graph.connect(this).is_err() {
            graph.arm_retry();
        }
    }
}

/// Default metadata values are JSON such as `{"name":"alsa_input.pci-0000_00_1f.3"}`.
pub fn default_name(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// An `EnumFormat` pod. Zero rate or channels leave them to the graph, so a capture
/// receives the device format and converts it with the core DSP like Windows.
fn format_pod(
    format: pw::spa::param::audio::AudioFormat,
    rate: u32,
    channels: u32,
) -> Option<Vec<u8>> {
    use pw::spa::param::audio::{AudioInfoRaw, MAX_CHANNELS};
    use pw::spa::sys;
    let mut info = AudioInfoRaw::new();
    info.set_format(format);
    if rate > 0 {
        info.set_rate(rate);
    }
    if channels > 0 {
        info.set_channels(channels);
        let mut position = [0; MAX_CHANNELS];
        match channels {
            1 => position[0] = sys::SPA_AUDIO_CHANNEL_MONO,
            _ => {
                position[0] = sys::SPA_AUDIO_CHANNEL_FL;
                position[1] = sys::SPA_AUDIO_CHANNEL_FR;
            }
        }
        info.set_position(position);
    }
    let object = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(object),
    )
    .ok()
    .map(|(cursor, _)| cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metadata_value_names_the_node() {
        assert_eq!(
            default_name(r#"{"name":"alsa_input.usb-mic"}"#).as_deref(),
            Some("alsa_input.usb-mic")
        );
        assert_eq!(default_name("not json"), None);
        assert_eq!(default_name(r#"{"other":1}"#), None);
    }

    #[test]
    fn speech_is_converted_to_the_virtual_mic_format() {
        let mut data = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 24000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut data, spec).unwrap();
        for sample in [0i16, 200, 100, 300, 400, 400] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        data.set_position(0);
        let pcm = Pcm::from_wav(hound::WavReader::new(data).unwrap()).unwrap();
        assert_eq!(pcm.to_mono(mic::RATE), [100, 150, 200, 300, 400, 400]);
        assert_eq!(pcm.to_mono(24000), [100, 200, 400]);
    }

    #[test]
    fn platform_objects_cross_threads() {
        fn shared<T: Send + Sync>() {}
        fn sent<T: Send>() {}
        shared::<Pipewire>();
        shared::<PipewireDevices>();
        shared::<PipewireSounds>();
        shared::<PipewirePlayer>();
        sent::<PipewireRecorder>();
    }

    // Without a daemon this exercises the thread and channel against a failed connection;
    // with one the listing succeeds instead. WSL lacks client.conf and the client modules.
    #[test]
    #[ignore = "needs the PipeWire client config and modules (Fedora pipewire-libs)"]
    fn commands_are_answered_with_or_without_a_daemon() {
        use dictto_core::platform::{Devices, Recorder};
        let pipewire = Pipewire::shared().unwrap();
        assert!(Arc::ptr_eq(&pipewire, &Pipewire::shared().unwrap()));
        let started = std::time::Instant::now();
        match PipewireDevices::new(pipewire.clone()).list_microphones() {
            Ok(list) => assert!(list.iter().all(|m| m.id != devices::VIRTUAL_MIC_NODE)),
            Err(error) => assert!(matches!(error, AudioError::DeviceError(_)), "{error}"),
        }
        assert!(started.elapsed() < Duration::from_secs(4));
        let mut recorder = PipewireRecorder::new(pipewire);
        assert!(matches!(recorder.stop(), Err(AudioError::NotRecording)));
    }

    #[test]
    #[ignore = "needs the PipeWire client config and modules (Fedora pipewire-libs)"]
    fn invalid_cache_file_reports_an_error_before_opening_a_stream() {
        use dictto_core::speech::Player;
        use futures_util::FutureExt;
        let path =
            std::env::temp_dir().join(format!("dictto-invalid-audio-{}.wav", std::process::id()));
        std::fs::write(&path, b"Invalid WAV").unwrap();
        let player = PipewirePlayer::new(Pipewire::shared().unwrap());
        let result = player
            .file(&path)
            .now_or_never()
            .expect("fails without waiting");
        std::fs::remove_file(&path).unwrap();
        assert!(result
            .unwrap_err()
            .contains("Cannot read cached speech WAV"));
    }
}
