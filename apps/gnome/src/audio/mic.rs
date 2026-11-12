//! The `dictto_microphone` node: an output stream classed as a virtual source, so other
//! applications capture it like a microphone. It sends silence until speech is queued.
use super::devices::VIRTUAL_MIC_NODE;
use super::{format_pod, Command, Pipewire};
use pipewire as pw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use tokio::sync::oneshot;

/// The node's fixed format is mono at this rate; speech is converted before queueing.
pub const RATE: u32 = 48_000;
pub const DESCRIPTION: &str = "Dictto Microphone";
pub(super) const MISSING: &str = "The Dictto Microphone is not available.";
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Feed {
    samples: Vec<i16>,
    position: usize,
    done: Option<oneshot::Sender<Result<(), String>>>,
}

impl Feed {
    /// Fills one buffer with queued speech, then silence.
    fn fill(&mut self, stream: &pw::stream::Stream) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };
        let requested = buffer.requested() as usize;
        let Some(data) = buffer.datas_mut().first_mut() else {
            return;
        };
        let mut frames = 0;
        if let Some(slice) = data.data() {
            frames = slice.len() / 2;
            if requested > 0 {
                frames = frames.min(requested);
            }
            let queued = frames.min(self.samples.len() - self.position);
            let speech = &self.samples[self.position..self.position + queued];
            let (chunks, _) = slice[..frames * 2].as_chunks_mut::<2>();
            for (index, out) in chunks.iter_mut().enumerate() {
                *out = speech.get(index).copied().unwrap_or(0).to_le_bytes();
            }
            self.position += queued;
            if queued > 0 && self.position == self.samples.len() {
                self.samples = Vec::new();
                self.position = 0;
                if let Some(done) = self.done.take() {
                    let _ = done.send(Ok(()));
                }
            }
        }
        let chunk = data.chunk_mut();
        *chunk.offset_mut() = 0;
        *chunk.stride_mut() = 2;
        *chunk.size_mut() = (frames * 2) as u32;
    }
}

/// The source stream on the PipeWire thread; dropping it removes the node.
pub(super) struct Source {
    /// Registry id of the node once announced; links are counted against it.
    pub(super) registered: Option<u32>,
    feed: Rc<RefCell<Feed>>,
    /// Set when the stream reports an error; such a stream never registers.
    failed: Rc<Cell<bool>>,
    // Declared before the stream so that the listener unregisters first.
    _listener: pw::stream::StreamListener<Rc<RefCell<Feed>>>,
    _stream: pw::stream::StreamRc,
}

impl Source {
    pub(super) fn open(core: &pw::core::CoreRc) -> Result<Self, String> {
        let format = format_pod(pw::spa::param::audio::AudioFormat::S16LE, RATE, 1);
        let format = format
            .as_deref()
            .and_then(pw::spa::pod::Pod::from_bytes)
            .ok_or("Cannot build the virtual microphone format.")?;
        let mut props = pw::properties::PropertiesBox::new();
        props.insert("media.type", "Audio");
        props.insert("media.class", "Audio/Source/Virtual");
        props.insert("node.name", VIRTUAL_MIC_NODE);
        props.insert("node.description", DESCRIPTION);
        props.insert("media.name", DESCRIPTION);
        let stream = pw::stream::StreamRc::new(core.clone(), VIRTUAL_MIC_NODE, props)
            .map_err(|e| format!("Cannot create the Dictto Microphone: {e}"))?;
        let feed = Rc::new(RefCell::new(Feed::default()));
        let failed = Rc::new(Cell::new(false));
        let listener = stream
            .add_local_listener_with_user_data(feed.clone())
            .process(|stream, feed| feed.borrow_mut().fill(stream))
            .state_changed({
                let failed = failed.clone();
                move |_, _, _, new| {
                    if let pw::stream::StreamState::Error(message) = new {
                        log::warn!("[pipewire] Dictto Microphone failed: {message}");
                        failed.set(true);
                    }
                }
            })
            .register()
            .map_err(|e| format!("Dictto Microphone listener: {e}"))?;
        // No AUTOCONNECT: the session manager must not link a source to a sink; consumers
        // link to it when they capture it.
        stream
            .connect(
                pw::spa::utils::Direction::Output,
                None,
                pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [format],
            )
            .map_err(|e| format!("Dictto Microphone connect: {e}"))?;
        Ok(Self {
            registered: None,
            feed,
            failed,
            _listener: listener,
            _stream: stream,
        })
    }

    pub(super) fn play(&self, samples: Vec<i16>, done: oneshot::Sender<Result<(), String>>) {
        let mut feed = self.feed.borrow_mut();
        if let Some(previous) = feed.done.take() {
            let _ = previous.send(Err("Speech playback was interrupted.".into()));
        }
        if samples.is_empty() {
            let _ = done.send(Ok(()));
            return;
        }
        feed.samples = samples;
        feed.position = 0;
        feed.done = Some(done);
    }

    pub(super) fn failed(&self) -> bool {
        self.failed.get()
    }

    pub(super) fn stop(&self) {
        *self.feed.borrow_mut() = Feed::default();
    }
}

fn reply<T>(answer: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
    answer
        .recv_timeout(REPLY_TIMEOUT)
        .map_err(|_| "PipeWire did not answer.".to_string())?
}

impl Pipewire {
    pub fn metadata_get(&self, key: &'static str) -> Result<Option<String>, String> {
        let (sender, answer) = mpsc::channel();
        self.send(Command::MetadataGet { key, reply: sender })
            .map_err(|e| e.to_string())?;
        reply(answer)
    }

    /// `None` deletes the key.
    pub fn metadata_set(&self, key: &'static str, value: Option<String>) -> Result<(), String> {
        let (sender, answer) = mpsc::channel();
        self.send(Command::MetadataSet { key, value, reply: sender })
            .map_err(|e| e.to_string())?;
        reply(answer)
    }

    /// Queues creation of the node unless it exists; the receiver answers once registered.
    pub fn mic_create(&self) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (sender, answer) = mpsc::channel();
        self.send(Command::MicCreate(sender))
            .map_err(|e| e.to_string())?;
        Ok(answer)
    }

    pub fn mic_destroy(&self) {
        let _ = self.send(Command::MicDestroy);
    }

    pub fn mic_links(&self) -> Result<usize, String> {
        let (sender, answer) = mpsc::channel();
        self.send(Command::MicLinks(sender))
            .map_err(|e| e.to_string())?;
        reply(answer)
    }

    /// Queues mono samples at `RATE`, replacing any queued speech.
    pub fn mic_play(&self, samples: Vec<i16>) -> Result<oneshot::Receiver<Result<(), String>>, String> {
        let (done, finished) = oneshot::channel();
        self.send(Command::MicPlay { samples, done })
            .map_err(|e| e.to_string())?;
        Ok(finished)
    }

    pub fn mic_stop(&self) {
        let _ = self.send(Command::MicStop);
    }
}
