//! Microphone and playback capture. The recorder runs on the caller's thread and waits
//! for the PipeWire thread, which owns the stream and runs the DSP in `process`.
use super::devices::NodeInfo;
use super::{format_pod, Command, Pipewire};
use dictto_core::audio::dsp::{finish, resample, to_mono_i16_f32, Waveform, SAMPLE_RATE};
use dictto_core::audio::{emit_waveform, AudioError, DeviceSelection};
use dictto_core::host::AppHandle;
use dictto_core::platform::Recorder;
use pipewire as pw;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// A monitor stream whose sink suspends stops delivering buffers; a gap longer than this
/// is kept as silence, like the Windows loopback capture keeps the pauses between sounds.
const LOOPBACK_GAP_TOLERANCE: usize = SAMPLE_RATE as usize / 4;

/// The error a capture ends with when the registry removes `removed` while it records
/// from `capturing` (both node names).
pub fn on_node_removed(capturing: &str, removed: &str) -> Option<AudioError> {
    (capturing == removed)
        .then(|| AudioError::DeviceError("The microphone was disconnected.".into()))
}

/// `Platform::new_recorder` builds one per runtime; all of them share the PipeWire thread.
pub struct PipewireRecorder {
    pipewire: Arc<Pipewire>,
    id: u64,
    app: Option<AppHandle>,
    recording: bool,
}

impl PipewireRecorder {
    pub fn new(pipewire: Arc<Pipewire>) -> Self {
        let id = pipewire.next_id();
        Self {
            pipewire,
            id,
            app: None,
            recording: false,
        }
    }
}

impl Recorder for PipewireRecorder {
    fn set_app_handle(&mut self, app: AppHandle) {
        self.app = Some(app);
    }

    fn start(&mut self, device: DeviceSelection) -> Result<(), AudioError> {
        if self.recording {
            return Err(AudioError::AlreadyRecording);
        }
        log::debug!("[PipewireRecorder] start() device={:?}", device);
        let (reply, result) = mpsc::channel();
        self.pipewire.send(Command::Capture {
            recorder: self.id,
            selection: device,
            app: self.app.clone(),
            reply,
        })?;
        match result.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => {
                self.recording = true;
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(_) => {
                let _ = self.pipewire.send(Command::CancelCapture(self.id));
                Err(AudioError::DeviceError(
                    "Start timed out: PipeWire is unresponsive".into(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<Vec<u8>, AudioError> {
        if !self.recording {
            return Err(AudioError::NotRecording);
        }
        self.recording = false;
        let (reply, result) = mpsc::channel();
        self.pipewire.send(Command::StopCapture {
            recorder: self.id,
            reply,
        })?;
        let samples = match result.recv_timeout(STOP_TIMEOUT) {
            Ok(samples) => samples?,
            Err(_) => {
                let _ = self.pipewire.send(Command::CancelCapture(self.id));
                return Err(AudioError::DeviceError(
                    "Stop timed out: PipeWire is unresponsive".into(),
                ));
            }
        };
        log::debug!(
            "[PipewireRecorder] Samples received: {} ({:.1}s at {}Hz)",
            samples.len(),
            samples.len() as f64 / SAMPLE_RATE as f64,
            SAMPLE_RATE,
        );
        finish(&samples)
    }

    fn cancel(&mut self) {
        if self.recording {
            self.recording = false;
            let _ = self.pipewire.send(Command::CancelCapture(self.id));
        }
    }

    fn is_recording(&self) -> bool {
        self.recording
    }
}

impl Drop for PipewireRecorder {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// A capture stream on the PipeWire thread.
pub(super) struct Capture {
    // Declared before the stream so that the listener unregisters first.
    listener: Option<pw::stream::StreamListener<()>>,
    stream: Option<pw::stream::StreamRc>,
    target: Option<String>,
    /// The default-sink generation a playback capture started with.
    sink_generation: Option<u64>,
    data: Rc<RefCell<CaptureData>>,
}

struct CaptureData {
    rate: u32,
    channels: usize,
    loopback: bool,
    started: Instant,
    samples: Vec<i16>,
    waveform: Waveform,
    app: Option<AppHandle>,
    error: Option<AudioError>,
}

impl Capture {
    /// `target` is the microphone node; `None` captures the default sink's monitor.
    pub(super) fn open(
        core: &pw::core::CoreRc,
        target: Option<NodeInfo>,
        sink_generation: Option<u64>,
        app: Option<AppHandle>,
    ) -> Result<Self, AudioError> {
        let failed = |e: pw::Error| AudioError::DeviceError(format!("PipeWire capture: {e}"));
        let mut props = pw::properties::PropertiesBox::new();
        props.insert("media.type", "Audio");
        props.insert("media.category", "Capture");
        props.insert("media.name", "Dictation");
        props.insert("node.name", "dictto-capture");
        match &target {
            // Pinned to the concrete node: neither a default change (the virtual mic
            // becoming the default) nor the node vanishing may move the stream.
            Some(node) => {
                props.insert("target.object", node.name.as_str());
                props.insert("node.dont-reconnect", "true");
            }
            None => props.insert("stream.capture.sink", "true"),
        }
        let stream =
            pw::stream::StreamRc::new(core.clone(), "dictto-capture", props).map_err(failed)?;
        let data = Rc::new(RefCell::new(CaptureData {
            rate: 0,
            channels: 0,
            loopback: target.is_none(),
            started: Instant::now(),
            samples: Vec::new(),
            waveform: Waveform::new(),
            app,
            error: None,
        }));
        let listener = stream
            .add_local_listener_with_user_data(())
            .param_changed({
                let data = data.clone();
                move |_, _, id, param| {
                    let Some(param) = param else { return };
                    if id != pw::spa::param::ParamType::Format.as_raw() {
                        return;
                    }
                    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
                    if info.parse(param).is_ok() {
                        log::info!(
                            "[PipewireRecorder] Device format: {}Hz, {}ch, f32",
                            info.rate(),
                            info.channels()
                        );
                        let mut data = data.borrow_mut();
                        data.rate = info.rate();
                        data.channels = info.channels() as usize;
                    }
                }
            })
            .state_changed({
                let data = data.clone();
                move |_, _, _, state| {
                    if let pw::stream::StreamState::Error(message) = state {
                        data.borrow_mut().fail(AudioError::DeviceError(message));
                    }
                }
            })
            .process({
                let data = data.clone();
                move |stream, _| {
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };
                    let Some(chunk) = buffer.datas_mut().first_mut() else {
                        return;
                    };
                    let offset = chunk.chunk().offset() as usize;
                    let size = chunk.chunk().size() as usize;
                    if let Some(bytes) = chunk.data() {
                        let end = offset.saturating_add(size).min(bytes.len());
                        data.borrow_mut().push(&bytes[offset.min(end)..end]);
                    }
                }
            })
            .register()
            .map_err(failed)?;
        let format = format_pod(pw::spa::param::audio::AudioFormat::F32LE, 0, 0)
            .ok_or_else(|| AudioError::DeviceError("Cannot build the capture format.".into()))?;
        let format = pw::spa::pod::Pod::from_bytes(&format)
            .ok_or_else(|| AudioError::DeviceError("Cannot build the capture format.".into()))?;
        stream
            .connect(
                pw::spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [format],
            )
            .map_err(failed)?;
        Ok(Self {
            listener: Some(listener),
            stream: Some(stream),
            target: target.map(|node| node.name),
            sink_generation,
            data,
        })
    }

    pub(super) fn node_removed(&self, removed: &str) {
        if let Some(error) = self
            .target
            .as_deref()
            .and_then(|t| on_node_removed(t, removed))
        {
            log::warn!("[PipewireRecorder] Capture node {removed} was removed");
            self.data.borrow_mut().fail(error);
        }
    }

    pub(super) fn fail(&mut self, error: AudioError) {
        self.close();
        self.data.borrow_mut().fail(error);
    }

    fn close(&mut self) {
        self.listener.take();
        self.stream.take();
    }

    pub(super) fn finish(mut self, sink_generation: u64) -> Result<Vec<i16>, AudioError> {
        self.close();
        let mut data = self.data.borrow_mut();
        if let Some(error) = data.error.take() {
            return Err(error);
        }
        if self.sink_generation.is_some_and(|g| g != sink_generation) {
            return Err(AudioError::DeviceError(
                "Default playback device changed. Record the sample again.".into(),
            ));
        }
        if data.loopback && !data.samples.is_empty() {
            let elapsed = elapsed_samples(data.started);
            let len = data.samples.len().max(elapsed);
            data.samples.resize(len, 0);
        }
        log::info!(
            "[PipewireRecorder] Capture stopped: {:.1}s, {} samples",
            data.started.elapsed().as_secs_f64(),
            data.samples.len()
        );
        Ok(std::mem::take(&mut data.samples))
    }
}

impl CaptureData {
    fn fail(&mut self, error: AudioError) {
        self.error.get_or_insert(error);
    }

    /// Same order as the Windows capture loop: mono at the device rate feeds the
    /// waveform, the 16 kHz resample feeds the recording.
    fn push(&mut self, bytes: &[u8]) {
        if self.error.is_some() || self.channels == 0 || self.rate == 0 {
            return;
        }
        let frames: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let samples = to_mono_i16_f32(&frames, self.channels);
        if samples.is_empty() {
            return;
        }
        let resampled =
            (self.rate != SAMPLE_RATE).then(|| resample(&samples, self.rate, SAMPLE_RATE));
        let resampled = resampled.as_deref().unwrap_or(&samples);
        if self.loopback {
            let start = elapsed_samples(self.started).saturating_sub(resampled.len());
            if start > self.samples.len() + LOOPBACK_GAP_TOLERANCE {
                self.samples.resize(start, 0);
            }
        }
        let bars = self.waveform.push(&samples, resampled.len());
        self.samples.extend_from_slice(resampled);
        if let (Some(bars), Some(app)) = (bars, &self.app) {
            emit_waveform(app, bars);
        }
    }
}

fn elapsed_samples(started: Instant) -> usize {
    (started.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_node_ends_capture_with_device_lost() {
        let error = on_node_removed("alsa_input.usb-mic", "alsa_input.usb-mic").unwrap();
        // The core maps device errors to the "microphone unavailable" message.
        assert!(matches!(error, AudioError::DeviceError(_)));
        assert!(error
            .to_string()
            .to_lowercase()
            .contains("audio device error"));
        assert!(on_node_removed("alsa_input.usb-mic", "alsa_input.pci").is_none());
    }

    fn capture_data(rate: u32, channels: usize) -> CaptureData {
        CaptureData {
            rate,
            channels,
            loopback: false,
            started: Instant::now(),
            samples: Vec::new(),
            waveform: Waveform::new(),
            app: None,
            error: None,
        }
    }

    fn bytes(frames: &[f32]) -> Vec<u8> {
        frames.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    #[test]
    fn stereo_48k_float_becomes_16k_mono() {
        let mut data = capture_data(48000, 2);
        data.push(&bytes(&[0.5; 960 * 2]));
        assert_eq!(data.samples.len(), 320);
        assert!(data
            .samples
            .iter()
            .all(|&s| s == (0.5 * i16::MAX as f32) as i16));
    }

    #[test]
    fn samples_after_a_failure_are_dropped() {
        let mut data = capture_data(16000, 1);
        data.push(&bytes(&[0.1; 160]));
        data.fail(on_node_removed("a", "a").unwrap());
        data.push(&bytes(&[0.1; 160]));
        assert_eq!(data.samples.len(), 160);
    }
}
