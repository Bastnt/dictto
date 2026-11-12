//! Playback side of the virtual microphone. The route policy (journal, manual change,
//! restore, suspend) is the core's `MicRoute`; this file owns the node's lifetime, which
//! follows the "Send speech to Dictto Microphone" setting.
use super::defaults::PipewireDefaults;
use crate::audio::{mic, Pcm, Pipewire};
use async_trait::async_trait;
use dictto_core::speech::route::Defaults;
use dictto_core::speech::virtual_mic::{virtual_mic, MicOutput, MicRoute};
use dictto_core::speech::{MicRouteControl, VirtualMic};
use std::io::Read;
use std::path::PathBuf;
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
    fn enable(&self) {
        let mut enabled = self.enabled.lock().unwrap();
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

/// The core route plus the node lifetime: the node exists only while routing is enabled.
pub struct GnomeMicRoute<D, N> {
    route: Arc<MicRoute<D, NodeOutput<N>>>,
    shared: Arc<Shared<N>>,
}

impl<D: Defaults + Send + Sync + 'static, N: Node> MicRouteControl for GnomeMicRoute<D, N> {
    fn recover(&self) -> Result<(), String> {
        self.route.recover()
    }
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            self.shared.enable();
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
pub fn route<D: Defaults + Send + Sync + 'static, N: Node>(
    journal: Result<PathBuf, String>,
    defaults: D,
    node: N,
) -> (Arc<dyn VirtualMic>, Arc<dyn MicRouteControl>) {
    let shared = Arc::new(Shared { node, enabled: Mutex::new(false) });
    let route = Arc::new(MicRoute::new(journal, defaults, NodeOutput(shared.clone())));
    (virtual_mic(&route), Arc::new(GnomeMicRoute { route, shared }))
}

pub fn pipewire_route(
    journal: Result<PathBuf, String>,
    pipewire: Arc<Pipewire>,
) -> (Arc<dyn VirtualMic>, Arc<dyn MicRouteControl>) {
    let defaults = PipewireDefaults::new(super::defaults::PipewireMetadata(pipewire.clone()));
    route(journal, defaults, PipewireNode(pipewire))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeNode {
        present: Mutex<bool>,
        calls: Mutex<Vec<&'static str>>,
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

    struct Fixed;
    impl Defaults for Fixed {
        fn current(&self) -> Result<String, String> {
            Ok("usb".into())
        }
        fn target(&self) -> Result<String, String> {
            Ok("dictto_microphone".into())
        }
        fn set(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
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
        let node = Arc::new(FakeNode::default());
        let (mic, control) = route(Err("no journal".into()), Fixed, node.clone());
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
