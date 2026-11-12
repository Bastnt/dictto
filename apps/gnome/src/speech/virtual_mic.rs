//! Playback side of the virtual microphone. The route policy (journal, manual change,
//! restore, suspend) is the core's `MicRoute`; this file owns the node's lifetime, which
//! follows the "Send speech to Dictto Microphone" setting.
use super::defaults::{GnomeDefaults, PipewireDefaults, PipewireMetadata};
use crate::audio::{mic, Pcm, Pipewire};
use async_trait::async_trait;
use dictto_core::speech::route;
use dictto_core::speech::virtual_mic::{virtual_mic, MicOutput, MicRoute};
use dictto_core::speech::{MicRouteControl, VirtualMic};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// How long capturing applications get to open the node after the default changes.
pub const READY_TIMEOUT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(50);
const CREATE_TIMEOUT: Duration = Duration::from_secs(3);
/// Past the audio length, after which a consumer that stopped pulling is given up.
const DRAIN_SLACK: Duration = Duration::from_secs(5);
const OFF: &str = "Virtual microphone routing is turned off.";
const NO_READER: &str =
    "No application opened the Dictto Microphone. Select it or the default microphone in the receiving app.";

/// The node operations, so the lifetime and timing rules are testable without PipeWire.
pub trait Node: Send + Sync + 'static {
    /// Creates the node unless it exists; the receiver answers once it is registered.
    fn create(&self) -> Result<mpsc::Receiver<Result<(), String>>, String>;
    fn destroy(&self);
    fn links(&self) -> Result<usize, String>;
    /// Queues mono samples at `mic::RATE`; the receiver answers once they were consumed.
    fn play(&self, samples: Vec<i16>) -> Result<oneshot::Receiver<Result<(), String>>, String>;
    fn stop(&self);
}

pub struct PipewireNode(pub Arc<Pipewire>);

impl Node for PipewireNode {
    fn create(&self) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        self.0.mic_create()
    }
    fn destroy(&self) {
        self.0.mic_destroy();
    }
    fn links(&self) -> Result<usize, String> {
        self.0.mic_links()
    }
    fn play(&self, samples: Vec<i16>) -> Result<oneshot::Receiver<Result<(), String>>, String> {
        self.0.mic_play(samples)
    }
    fn stop(&self) {
        self.0.mic_stop();
    }
}

struct Shared<N> {
    node: N,
    /// Held while a create or destroy is queued so they reach PipeWire in setting order.
    enabled: Mutex<bool>,
}

impl<N: Node> Shared<N> {
    /// `turning_on` runs only when routing was off, before the node is created; no
    /// route can be active then, since the core route is enabled after this.
    fn enable(&self, turning_on: impl FnOnce()) {
        let mut enabled = self.enabled.lock().unwrap();
        if !*enabled {
            turning_on();
        }
        *enabled = true;
        // Not awaited: a slow PipeWire must not hold up the runtime start; a failure
        // surfaces when speech is next routed.
        if let Err(error) = self.node.create() {
            log::warn!("Cannot create the Dictto Microphone: {error}");
        }
    }
    fn disable(&self) {
        let mut enabled = self.enabled.lock().unwrap();
        *enabled = false;
        self.node.destroy();
    }
    fn ensure(&self) -> Result<(), String> {
        let pending = {
            let enabled = self.enabled.lock().unwrap();
            if !*enabled {
                return Err(OFF.into());
            }
            self.node.create()?
        };
        pending
            .recv_timeout(CREATE_TIMEOUT)
            .map_err(|_| "PipeWire did not create the Dictto Microphone.".to_string())?
    }
}

pub struct NodeOutput<N>(Arc<Shared<N>>);

/// Stops queued speech when a playback is abandoned before it finished.
struct StopGuard<'a, N: Node>(Option<&'a N>);
impl<N: Node> Drop for StopGuard<'_, N> {
    fn drop(&mut self) {
        if let Some(node) = self.0 {
            node.stop();
        }
    }
}

/// Returns once `links` reports a consumer, `check` fails, or `limit` passes.
async fn wait_for_link<C>(
    mut links: impl FnMut() -> Result<usize, String> + Send,
    mut check: C,
    limit: Duration,
) -> Result<(), String>
where
    C: FnMut() -> Result<(), String> + Send,
{
    let deadline = Instant::now() + limit;
    loop {
        if links()? > 0 {
            return Ok(());
        }
        check()?;
        if Instant::now() >= deadline {
            return Err(NO_READER.into());
        }
        tokio::time::sleep(POLL).await;
    }
}

#[async_trait]
impl<N: Node> MicOutput for NodeOutput<N> {
    type Target = ();
    fn ensure_present(&self) -> Result<(), String> {
        self.0.ensure()
    }
    /// Recreates the node if PipeWire restarted since the availability check.
    fn locate(&self) -> Result<(), String> {
        self.0.ensure()
    }
    async fn wait_ready<C>(&self, check: C) -> Result<(), String>
    where
        C: FnMut() -> Result<(), String> + Send,
    {
        wait_for_link(|| self.0.node.links(), check, READY_TIMEOUT).await
    }
    async fn play<R, C>(&self, wav: hound::WavReader<R>, _: &(), mut check: C) -> Result<(), String>
    where
        R: Read + Send,
        C: FnMut() -> Result<(), String> + Send,
    {
        let samples = Pcm::from_wav(wav)?.to_mono(mic::RATE);
        let length = Duration::from_secs_f64(samples.len() as f64 / f64::from(mic::RATE));
        let node = &self.0.node;
        let mut done = node.play(samples)?;
        let mut guard = StopGuard(Some(node));
        let deadline = Instant::now() + length + DRAIN_SLACK;
        loop {
            tokio::select! {
                result = &mut done => {
                    guard.0 = None;
                    return result.unwrap_or_else(|_| Err("Speech playback was interrupted.".into()));
                }
                () = tokio::time::sleep(POLL) => {
                    check()?;
                    if Instant::now() >= deadline {
                        return Err(NO_READER.into());
                    }
                }
            }
        }
    }
}

pub type PipewireMicOutput = NodeOutput<PipewireNode>;

/// Journal name of builds that kept one journal per logind session.
const LEGACY_PREFIX: &str = "microphone-route-";

/// The core route plus the node lifetime: the node exists only while routing is enabled.
pub struct GnomeMicRoute<D, N> {
    route: Arc<MicRoute<D, NodeOutput<N>>>,
    shared: Arc<Shared<N>>,
    defaults: D,
    journal: PathBuf,
    legacy_checked: AtomicBool,
}

impl<D: GnomeDefaults, N: Node> GnomeMicRoute<D, N> {
    /// Restores routes journaled per logind session. Session ids are never reused, so
    /// nothing else would ever read those files.
    fn recover_legacy(&self) {
        let Some(dir) = self.journal.parent() else { return };
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let legacy = path.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                name.starts_with(LEGACY_PREFIX) && name.ends_with(".json")
            });
            if !legacy {
                continue;
            }
            match route::recover(&path, &self.defaults) {
                Ok(()) => log::info!("recovered the microphone route from {}", path.display()),
                Err(error) => {
                    // Deleted anyway: a configured virtual mic left behind is cleared
                    // when routing is next enabled.
                    log::warn!("cannot recover {}: {error}", path.display());
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    /// A configured `dictto_microphone` without a journal was left by a route whose
    /// restore never ran; once the node exists again it would silently capture the user.
    fn forget_stale_default(&self) {
        if self.journal.exists() {
            return;
        }
        match self.defaults.forget_virtual() {
            Ok(true) => log::info!("cleared a stale Dictto Microphone default"),
            Ok(false) => {}
            Err(error) => log::warn!("cannot check the default microphone: {error}"),
        }
    }
}

impl<D: GnomeDefaults, N: Node> MicRouteControl for GnomeMicRoute<D, N> {
    fn recover(&self) -> Result<(), String> {
        let result = self.route.recover();
        if !self.legacy_checked.swap(true, Ordering::AcqRel) {
            self.recover_legacy();
        }
        result
    }
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            self.shared.enable(|| self.forget_stale_default());
            self.route.set_enabled(true)
        } else {
            // Restored before the node goes, so the default never names a missing node.
            let restored = self.route.set_enabled(false);
            self.shared.disable();
            restored
        }
    }
    fn session_changed(&self, suspended: bool) -> Result<(), String> {
        self.route.session_changed(suspended)
    }
    fn shutdown(&self) {
        self.route.shutdown();
        self.shared.disable();
    }
    fn input_name(&self) -> String {
        MicRouteControl::input_name(&*self.route)
    }
}

/// The `VirtualMic` for `SpeechPlatform` and the lifecycle control the runtime drives.
/// PipeWire metadata and the session bus are per user and so is the daemon, hence one
/// journal per user.
pub fn route<D: GnomeDefaults, N: Node>(
    journal: PathBuf,
    defaults: D,
    node: N,
) -> (Arc<dyn VirtualMic>, Arc<dyn MicRouteControl>) {
    let shared = Arc::new(Shared { node, enabled: Mutex::new(false) });
    let route = Arc::new(MicRoute::new(Ok(journal.clone()), defaults.clone(), NodeOutput(shared.clone())));
    let control = GnomeMicRoute { route: route.clone(), shared, defaults, journal, legacy_checked: AtomicBool::new(false) };
    (virtual_mic(&route), Arc::new(control))
}

pub fn pipewire_route(journal: PathBuf, pipewire: Arc<Pipewire>) -> (Arc<dyn VirtualMic>, Arc<dyn MicRouteControl>) {
    let defaults = PipewireDefaults::new(PipewireMetadata(pipewire.clone()));
    route(journal, defaults, PipewireNode(pipewire))
}

#[cfg(test)]
mod tests {
    use super::super::defaults::fake::Store;
    use super::super::defaults::{metadata_value, MetadataStore, CONFIGURED_SOURCE};
    use super::*;
    use crate::audio::devices::VIRTUAL_MIC_NODE;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct FakeNode {
        present: Mutex<bool>,
        /// Shared with the fake metadata store so the order of both is visible.
        calls: Arc<Mutex<Vec<&'static str>>>,
        links: AtomicUsize,
        /// The playback's completion, kept so the test decides when it finishes.
        playing: Mutex<Option<oneshot::Sender<Result<(), String>>>>,
    }
    impl Node for Arc<FakeNode> {
        fn create(&self) -> Result<mpsc::Receiver<Result<(), String>>, String> {
            self.calls.lock().unwrap().push("create");
            *self.present.lock().unwrap() = true;
            let (reply, answer) = mpsc::channel();
            reply.send(Ok(())).unwrap();
            Ok(answer)
        }
        fn destroy(&self) {
            self.calls.lock().unwrap().push("destroy");
            *self.present.lock().unwrap() = false;
        }
        fn links(&self) -> Result<usize, String> {
            Ok(self.links.load(Ordering::SeqCst))
        }
        fn play(&self, _: Vec<i16>) -> Result<oneshot::Receiver<Result<(), String>>, String> {
            let (done, finished) = oneshot::channel();
            *self.playing.lock().unwrap() = Some(done);
            Ok(finished)
        }
        fn stop(&self) {
            self.calls.lock().unwrap().push("stop");
        }
    }

    type Defaults = PipewireDefaults<Store>;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dictto-gnome-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    type Fixture = (Arc<FakeNode>, Store, PathBuf, Arc<dyn VirtualMic>, Arc<dyn MicRouteControl>);

    /// A route over a fake store whose writes land in the node's call log.
    fn fixture(name: &str, configured: Option<&str>) -> Fixture {
        let node = Arc::new(FakeNode::default());
        let mut store = Store::with_source(configured);
        store.log = node.calls.clone();
        let dir = temp_dir(name);
        let (mic, control) = route(dir.join("microphone-route.json"), Defaults::new(store.clone()), node.clone());
        (node, store, dir, mic, control)
    }

    fn output(node: &Arc<FakeNode>) -> NodeOutput<Arc<FakeNode>> {
        NodeOutput(Arc::new(Shared { node: node.clone(), enabled: Mutex::new(true) }))
    }

    fn wav(frames: u32) -> hound::WavReader<std::io::Cursor<Vec<u8>>> {
        let mut data = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut data, spec).unwrap();
        for _ in 0..frames {
            writer.write_sample(1i16).unwrap();
        }
        writer.finalize().unwrap();
        data.set_position(0);
        hound::WavReader::new(data).unwrap()
    }

    #[test]
    fn virtual_mic_hidden_when_setting_off() {
        let (node, _store, dir, mic, control) = fixture("hidden", Some("usb"));
        assert!(!*node.present.lock().unwrap());
        assert_eq!(mic.check_available().unwrap_err(), OFF);
        assert!(node.calls.lock().unwrap().is_empty());

        control.set_enabled(true).unwrap();
        assert!(*node.present.lock().unwrap());
        control.set_enabled(false).unwrap();
        assert!(!*node.present.lock().unwrap());
        assert_eq!(mic.check_available().unwrap_err(), OFF);
        // The node output refuses on its own too, so a check racing the setting cannot
        // bring the node back.
        let shared = Shared { node: node.clone(), enabled: Mutex::new(false) };
        assert_eq!(shared.ensure().unwrap_err(), OFF);
        assert_eq!(*node.calls.lock().unwrap(), ["create", "destroy"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn orphaned_session_journal_is_recovered_at_start() {
        let (_node, store, dir, _mic, control) = fixture("orphan", Some(VIRTUAL_MIC_NODE));
        let legacy = dir.join("microphone-route-c2.json");
        std::fs::write(&legacy, br#"{"original":"alsa_input.usb","temporary":"dictto_microphone"}"#).unwrap();
        control.recover().unwrap();
        assert_eq!(store.configured().as_deref(), Some("alsa_input.usb"));
        assert!(!legacy.exists());
        // Only once: a file appearing later is left to nobody, not re-read every start.
        std::fs::write(&legacy, b"{}").unwrap();
        control.recover().unwrap();
        assert!(legacy.exists());

        // An unrecoverable legacy journal is still removed.
        let (_node, _store, dir2, _mic, control) = fixture("orphan-bad", None);
        let broken = dir2.join("microphone-route-7.json");
        std::fs::write(&broken, b"not json").unwrap();
        control.recover().unwrap();
        assert!(!broken.exists());
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
    }

    #[test]
    fn stale_configured_virtual_mic_is_cleared_before_node_creation() {
        let (node, store, dir, _mic, control) = fixture("stale", Some(VIRTUAL_MIC_NODE));
        control.set_enabled(true).unwrap();
        assert_eq!(store.configured(), None);
        assert_eq!(*node.calls.lock().unwrap(), ["clear", "create"]);
        // Re-enabling while on never touches the default: a route may be active.
        store.set(CONFIGURED_SOURCE, Some(metadata_value(VIRTUAL_MIC_NODE))).unwrap();
        control.set_enabled(true).unwrap();
        assert_eq!(store.configured().as_deref(), Some(VIRTUAL_MIC_NODE));

        // With a journal the route owns the switch and recovery restores it instead.
        let (node, store, dir2, _mic, control) = fixture("journaled", Some(VIRTUAL_MIC_NODE));
        std::fs::write(dir2.join("microphone-route.json"), b"{}").unwrap();
        control.set_enabled(true).unwrap();
        assert_eq!(store.configured().as_deref(), Some(VIRTUAL_MIC_NODE));
        assert_eq!(*node.calls.lock().unwrap(), ["create"]);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn wait_ready_returns_on_a_link_and_aborts_on_check() {
        let node = Arc::new(FakeNode::default());
        let output = output(&node);
        node.links.store(1, Ordering::SeqCst);
        let started = Instant::now();
        output.wait_ready(|| Ok(())).await.unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);

        node.links.store(0, Ordering::SeqCst);
        let mut polls = 0;
        let error = output
            .wait_ready(|| {
                polls += 1;
                if polls == 3 {
                    Err("locked".to_string())
                } else {
                    Ok(())
                }
            })
            .await
            .unwrap_err();
        assert_eq!(error, "locked");
        assert_eq!(started.elapsed(), POLL * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_ready_gives_up_after_eight_seconds() {
        let node = Arc::new(FakeNode::default());
        let started = Instant::now();
        let error = output(&node).wait_ready(|| Ok(())).await.unwrap_err();
        assert_eq!(error, NO_READER);
        assert!(started.elapsed() >= READY_TIMEOUT);
        assert!(started.elapsed() < READY_TIMEOUT + POLL * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn failing_check_stops_queued_speech() {
        let node = Arc::new(FakeNode::default());
        let output = output(&node);
        let mut polls = 0;
        let error = output
            .play(wav(24000), &(), || {
                polls += 1;
                if polls == 2 { Err("changed".to_string()) } else { Ok(()) }
            })
            .await
            .unwrap_err();
        assert_eq!(error, "changed");
        assert_eq!(*node.calls.lock().unwrap(), ["stop"]);

        let finishing = node.clone();
        tokio::spawn(async move {
            tokio::time::sleep(POLL * 3).await;
            let done = finishing.playing.lock().unwrap().take().unwrap();
            done.send(Ok(())).unwrap();
        });
        output.play(wav(24000), &(), || Ok(())).await.unwrap();
        assert_eq!(*node.calls.lock().unwrap(), ["stop"]);
    }
}
