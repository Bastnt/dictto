//! Playback of 16-bit PCM through a PipeWire stream, for speech and UI sounds.
use super::{format_pod, Command, Pipewire};
use async_trait::async_trait;
use dictto_core::speech::Player;
use pipewire as pw;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Interleaved 16-bit samples, as read from a WAV.
#[derive(Clone)]
pub struct Pcm {
    samples: Vec<i16>,
    rate: u32,
    channels: u32,
}

impl Pcm {
    pub fn from_wav<R: Read>(mut reader: hound::WavReader<R>) -> Result<Self, String> {
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err("Speech playback requires a 16-bit PCM WAV.".into());
        }
        if !(1..=2).contains(&spec.channels) || spec.sample_rate == 0 {
            return Err("Speech playback requires a mono or stereo WAV.".into());
        }
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Invalid speech samples: {e}"))?;
        Ok(Self {
            samples,
            rate: spec.sample_rate,
            channels: spec.channels.into(),
        })
    }
}

pub(super) struct Request {
    pcm: Pcm,
    target: Option<String>,
    role: &'static str,
    done: Option<oneshot::Sender<Result<(), String>>>,
}

impl Request {
    pub(super) fn fail(self, message: &str) {
        if let Some(done) = self.done {
            let _ = done.send(Err(message.into()));
        }
    }
}

impl Pipewire {
    /// Plays `pcm`, on the node named `target` or the default sink. Dropping the returned
    /// handle before it finishes stops the playback.
    pub fn play(
        self: &Arc<Self>,
        pcm: Pcm,
        target: Option<String>,
        role: &'static str,
    ) -> Result<Playing, String> {
        let (done, finished) = oneshot::channel();
        let id = self.next_id();
        let request = Request {
            pcm,
            target,
            role,
            done: Some(done),
        };
        self.send(Command::Play { id, request })
            .map_err(|e| e.to_string())?;
        Ok(Playing {
            pipewire: self.clone(),
            id,
            finished,
            ended: false,
        })
    }

    /// Plays without waiting or reporting, for UI sounds.
    pub(super) fn play_detached(&self, pcm: Pcm, role: &'static str) {
        let request = Request {
            pcm,
            target: None,
            role,
            done: None,
        };
        let id = self.next_id();
        if let Err(error) = self.send(Command::Play { id, request }) {
            log::warn!("[pipewire] Sound not played: {error}");
        }
    }
}

pub struct Playing {
    pipewire: Arc<Pipewire>,
    id: u64,
    finished: oneshot::Receiver<Result<(), String>>,
    ended: bool,
}

impl Playing {
    /// Resolves once the stream has drained.
    pub async fn wait(&mut self) -> Result<(), String> {
        let result = (&mut self.finished).await;
        self.ended = true;
        result.unwrap_or_else(|_| Err("Speech playback was interrupted.".into()))
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.pipewire.send(Command::StopPlayback(self.id));
        }
    }
}

/// Slack past the audio length before a playback that never drains (no sink to link
/// to) is dropped, which ends its waiter with an interruption.
const DRAIN_SLACK: Duration = Duration::from_secs(5);

/// A playback stream on the PipeWire thread.
pub(super) struct Playback {
    _deadline: pw::loop_::TimerSource<'static>,
    // Declared before the stream so that the listener unregisters first.
    _listener: pw::stream::StreamListener<State>,
    _stream: pw::stream::StreamRc,
}

struct State {
    id: u64,
    pcm: Pcm,
    position: usize,
    flushed: bool,
    done: Option<oneshot::Sender<Result<(), String>>>,
    commands: pw::channel::Sender<Command>,
}

impl State {
    /// The stream is dropped through the command queue: destroying it inside its own
    /// callback is not allowed.
    fn finish(&mut self, result: Result<(), String>) {
        if let Some(done) = self.done.take() {
            let _ = done.send(result);
        }
        let _ = self.commands.send(Command::PlaybackEnded(self.id));
    }

    /// Fills one buffer; returns true once every sample has been queued.
    fn fill(&mut self, stream: &pw::stream::Stream) -> bool {
        let channels = self.pcm.channels as usize;
        let stride = 2 * channels;
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return false;
        };
        let requested = buffer.requested() as usize;
        let Some(data) = buffer.datas_mut().first_mut() else {
            return false;
        };
        let mut frames = 0;
        if let Some(slice) = data.data() {
            frames = slice.len() / stride;
            if requested > 0 {
                frames = frames.min(requested);
            }
            frames = frames.min((self.pcm.samples.len() - self.position) / channels);
            let samples = &self.pcm.samples[self.position..self.position + frames * channels];
            for (out, sample) in slice.as_chunks_mut::<2>().0.iter_mut().zip(samples) {
                *out = sample.to_le_bytes();
            }
            self.position += frames * channels;
        }
        let chunk = data.chunk_mut();
        *chunk.offset_mut() = 0;
        *chunk.stride_mut() = stride as i32;
        *chunk.size_mut() = (frames * stride) as u32;
        self.position + channels > self.pcm.samples.len()
    }
}

impl Playback {
    /// Reports a failure through the request. Once the stream exists the request's
    /// sender lives in the listener, and dropping it reads as an interruption.
    pub(super) fn open(
        core: &pw::core::CoreRc,
        loop_: &'static pw::loop_::Loop,
        id: u64,
        request: Request,
        commands: pw::channel::Sender<Command>,
    ) -> Result<Self, String> {
        let format = format_pod(
            pw::spa::param::audio::AudioFormat::S16LE,
            request.pcm.rate,
            request.pcm.channels,
        );
        let Some(format) = format.as_deref().and_then(pw::spa::pod::Pod::from_bytes) else {
            let message = "Cannot build the playback format.";
            request.fail(message);
            return Err(message.into());
        };
        let mut props = pw::properties::PropertiesBox::new();
        props.insert("media.type", "Audio");
        props.insert("media.category", "Playback");
        props.insert("media.role", request.role);
        props.insert("node.name", "dictto-playback");
        if let Some(target) = &request.target {
            props.insert("target.object", target.as_str());
        }
        let stream = match pw::stream::StreamRc::new(core.clone(), "dictto-playback", props) {
            Ok(stream) => stream,
            Err(e) => {
                let message = format!("Cannot open a PipeWire playback stream: {e}");
                request.fail(&message);
                return Err(message);
            }
        };
        let length = Duration::from_secs_f64(
            request.pcm.samples.len() as f64 / f64::from(request.pcm.channels * request.pcm.rate),
        );
        let deadline = loop_.add_timer({
            let commands = commands.clone();
            move |_| {
                log::warn!("[pipewire] Playback {id} did not drain; dropping it");
                let _ = commands.send(Command::PlaybackEnded(id));
            }
        });
        let _ = deadline.update_timer(Some(length + DRAIN_SLACK), None);
        let state = State {
            id,
            pcm: request.pcm,
            position: 0,
            flushed: false,
            done: request.done,
            commands,
        };
        let listener = stream
            .add_local_listener_with_user_data(state)
            .process(|stream, state| {
                if state.fill(stream) && !state.flushed {
                    state.flushed = true;
                    if let Err(e) = stream.flush(true) {
                        state.finish(Err(format!("Speech playback failed: {e}")));
                    }
                }
            })
            .drained(|_, state| state.finish(Ok(())))
            .state_changed(|_, state, _, new| {
                if let pw::stream::StreamState::Error(message) = new {
                    state.finish(Err(format!("Speech playback failed: {message}")));
                }
            })
            .register()
            .map_err(|e| format!("PipeWire playback listener: {e}"))?;
        stream
            .connect(
                pw::spa::utils::Direction::Output,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [format],
            )
            .map_err(|e| format!("PipeWire playback connect: {e}"))?;
        Ok(Self {
            _deadline: deadline,
            _listener: listener,
            _stream: stream,
        })
    }
}

/// Speech output on the default sink.
pub struct PipewirePlayer {
    pipewire: Arc<Pipewire>,
}

impl PipewirePlayer {
    pub fn new(pipewire: Arc<Pipewire>) -> Self {
        Self { pipewire }
    }

    async fn play(&self, pcm: Pcm) -> Result<(), String> {
        if pcm.samples.is_empty() {
            return Ok(());
        }
        self.pipewire.play(pcm, None, "Accessibility")?.wait().await
    }
}

#[async_trait]
impl Player for PipewirePlayer {
    async fn chunk(&self, audio: Vec<u8>) -> Result<(), String> {
        let reader = hound::WavReader::new(audio.as_slice())
            .map_err(|e| format!("Invalid speech WAV: {e}"))?;
        self.play(Pcm::from_wav(reader)?).await
    }

    async fn file(&self, path: &Path) -> Result<(), String> {
        let reader = hound::WavReader::open(path)
            .map_err(|e| format!("Cannot read cached speech WAV: {e}"))?;
        self.play(Pcm::from_wav(reader)?).await
    }
}
