use super::communications::{self, Defaults, WindowsDefaults};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::DeviceIoControl;

const WRITE: u32 = 0x8000a004;
const FLUSH: u32 = 0x8000a008;
const DEPTH: u32 = 0x8000a00c;
const ACTIVE: u32 = 0x8000a010;
const RATE: u32 = 48000;
const POLL: Duration = Duration::from_millis(10);

struct Feed(File);
impl Feed {
    fn open() -> Result<Self, String> {
        let file=OpenOptions::new().write(true).share_mode(0).open(r"\\.\DicttoMicFeed")
            .map_err(|e|format!("Cannot open the Dictto virtual microphone driver. Check that it is installed and available ({e})."))?;
        Ok(Self(file))
    }
    fn ioctl(&self, code: u32, input: &[i16], output: Option<&mut u32>) -> io::Result<()> {
        let out_len = if output.is_some() { 4 } else { 0 };
        let mut returned = 0;
        unsafe {
            DeviceIoControl(
                HANDLE(self.0.as_raw_handle()),
                code,
                (!input.is_empty()).then(|| input.as_ptr().cast()),
                std::mem::size_of_val(input) as u32,
                output.map(|v| (v as *mut u32).cast()),
                out_len,
                Some(&mut returned),
                None,
            )
            .map_err(|e| io::Error::from_raw_os_error((e.code().0 as u32 & 0xffff) as i32))?;
        }
        if returned != out_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Dictto driver response length",
            ));
        }
        Ok(())
    }
    fn status(&self, code: u32) -> io::Result<u32> {
        let mut value = 0;
        self.ioctl(code, &[], Some(&mut value))?;
        Ok(value)
    }
}

struct Active {
    token: u64,
    target: String,
    feed: Feed,
    checked: Instant,
}
#[derive(Default)]
struct State {
    generation: u64,
    closed: bool,
    suspended: bool,
    enabled: bool,
    active: Option<Active>,
}

pub(crate) struct Manager {
    journal: Result<PathBuf, String>,
    state: Mutex<State>,
}
impl Manager {
    pub(crate) fn new(data_dir: &Path) -> Self {
        let mut session = 0;
        let journal = unsafe {
            windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                std::process::id(),
                &mut session,
            )
        }
        .map(|()| data_dir.join(format!("communications-route-{session}.json")))
        .map_err(|e| e.to_string());
        Self {
            journal,
            state: Mutex::new(State::default()),
        }
    }
    fn journal(&self) -> Result<&Path, String> {
        self.journal.as_deref().map_err(Clone::clone)
    }
    pub(crate) fn recover(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if state.active.is_some() {
            return Ok(());
        }
        communications::recover(self.journal()?, &WindowsDefaults)
    }
    pub(crate) fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.state.lock().unwrap().enabled = enabled;
        if enabled {
            Ok(())
        } else {
            self.end(None, false)
        }
    }
    pub(super) fn check_available(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if !state.enabled {
            return Err("Virtual microphone routing is turned off.".into());
        }
        if state.suspended || state.closed {
            return Err(
                "Virtual microphone routing requires an active, unlocked desktop session.".into(),
            );
        }
        WindowsDefaults.target()?;
        WindowsDefaults.current()?;
        Ok(())
    }
    fn begin(&self) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("Dictto is shutting down.".into());
        }
        if state.suspended {
            return Err(
                "Virtual microphone routing requires an active, unlocked desktop session.".into(),
            );
        }
        if state.active.is_some() {
            return Err("Another virtual microphone action is still active.".into());
        }
        if !state.enabled {
            return Err("Virtual microphone routing was turned off.".into());
        }
        let feed = Feed::open()?;
        let target = communications::acquire(self.journal()?, &WindowsDefaults)?;
        state.generation += 1;
        let token = state.generation;
        state.active = Some(Active {
            token,
            target,
            feed,
            checked: Instant::now(),
        });
        Ok(token)
    }
    fn call<T>(
        &self,
        token: u64,
        check_now: bool,
        f: impl FnOnce(&Feed) -> io::Result<T>,
    ) -> Result<io::Result<T>, String> {
        let mut state = self.state.lock().unwrap();
        let active = state
            .active
            .as_mut()
            .filter(|a| a.token == token)
            .ok_or("Virtual microphone playback was stopped.")?;
        if check_now || active.checked.elapsed() >= Duration::from_millis(100) {
            if WindowsDefaults.current()? != active.target {
                return Err(
                    "The communications microphone changed. Dictto stopped routing speech.".into(),
                );
            }
            active.checked = Instant::now();
        }
        Ok(f(&active.feed))
    }
    fn end(&self, token: Option<u64>, closed: bool) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.closed |= closed;
        if state
            .active
            .as_ref()
            .is_none_or(|a| token.is_some_and(|t| t != a.token))
        {
            return Ok(());
        }
        let active = state.active.take().unwrap();
        let flush = active.feed.ioctl(FLUSH, &[], None);
        drop(active);
        communications::recover(self.journal()?, &WindowsDefaults)?;
        flush.map_err(|e| format!("Cannot flush Dictto microphone: {e}"))
    }
    pub(crate) fn session_changed(&self, suspended: bool) -> Result<(), String> {
        self.state.lock().unwrap().suspended = suspended;
        if suspended {
            self.end(None, false)
        } else {
            self.recover()
        }
    }
    pub(crate) fn shutdown(&self) {
        if let Err(error) = self.end(None, true) {
            log::error!("Communications microphone recovery pending: {error}");
        }
    }
}

pub(super) struct Player {
    manager: Arc<Manager>,
    token: Mutex<Option<u64>>,
}
impl Player {
    pub(super) fn new(manager: Arc<Manager>) -> Self {
        Self {
            manager,
            token: Mutex::new(None),
        }
    }
    fn token(&self) -> Result<u64, String> {
        let mut token = self.token.lock().unwrap();
        if let Some(value) = *token {
            return Ok(value);
        }
        let value = self.manager.begin()?;
        *token = Some(value);
        Ok(value)
    }
    pub(super) async fn play<R: Read + Send>(
        &self,
        wav: hound::WavReader<R>,
    ) -> Result<(), String> {
        let source = Mono48::new(wav)?;
        if source.frames == 0 {
            return Err("Empty speech WAV.".into());
        }
        let token = self.token()?;
        self.manager
            .call(token, true, |_| Ok(()))?
            .map_err(|e| e.to_string())?;
        pump(
            source,
            &LiveSink {
                manager: &self.manager,
                token,
            },
            Duration::from_secs(8),
        )
        .await
    }
    pub(super) fn finish(&self) -> Result<(), String> {
        let token = self.token.lock().unwrap().take();
        if let Some(value) = token {
            self.manager.end(Some(value), false)
        } else {
            Ok(())
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            log::error!("Microphone restoration pending: {error}");
        }
    }
}

trait Sink {
    fn active(&self) -> Result<bool, String>;
    fn write(&self, samples: &[i16]) -> Result<bool, String>;
    fn queued(&self) -> Result<u32, String>;
}
struct LiveSink<'a> {
    manager: &'a Manager,
    token: u64,
}
impl Sink for LiveSink<'_> {
    fn active(&self) -> Result<bool, String> {
        self.manager
            .call(self.token, false, |feed| feed.status(ACTIVE))?
            .map(|v| v != 0)
            .map_err(|e| e.to_string())
    }
    fn write(&self, samples: &[i16]) -> Result<bool, String> {
        match self
            .manager
            .call(self.token, false, |feed| feed.ioctl(WRITE, samples, None))?
        {
            Ok(()) => Ok(true),
            Err(e) if e.raw_os_error() == Some(170) => Ok(false),
            Err(e) => Err(format!(
                "Dictto Microphone stopped receiving audio. Check the call's input device ({e})."
            )),
        }
    }
    fn queued(&self) -> Result<u32, String> {
        self.manager
            .call(self.token, false, |feed| feed.status(DEPTH))?
            .map_err(|e| format!("Dictto Microphone stopped before the audio drained: {e}"))
    }
}

async fn pump<R: Read>(
    mut source: Mono48<R>,
    sink: &impl Sink,
    ready_timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    while !sink.active()? {
        if start.elapsed() >= ready_timeout {
            return Err("No app opened Dictto Microphone. Start the Discord call and use its default input, then try again.".into());
        }
        tokio::time::sleep(POLL).await;
    }
    let mut packet = Vec::with_capacity(960);
    loop {
        source.fill(&mut packet)?;
        if packet.is_empty() {
            break;
        }
        let start = Instant::now();
        while !sink.write(&packet)? {
            if start.elapsed() >= Duration::from_secs(2) {
                return Err("The Dictto microphone stopped consuming speech.".into());
            }
            tokio::time::sleep(POLL).await;
        }
    }
    let start = Instant::now();
    while sink.queued()? != 0 {
        if start.elapsed() >= Duration::from_secs(2) {
            return Err("Timed out draining the Dictto microphone.".into());
        }
        tokio::time::sleep(POLL).await;
    }
    // Give the capture client a final processing period after the DMA queue is empty.
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

/// Keep two input frames for linear interpolation. Files and newly synthesized chunks use the same bounded converter.
struct Mono48<R: Read> {
    reader: hound::WavReader<R>,
    channels: u16,
    rate: u32,
    left: i16,
    right: i16,
    input_frame: u64,
    frames: u64,
    position: u64,
}
impl<R: Read> Mono48<R> {
    fn new(reader: hound::WavReader<R>) -> Result<Self, String> {
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Int
            || spec.bits_per_sample != 16
            || spec.channels == 0
            || spec.sample_rate == 0
        {
            return Err("Virtual microphone playback requires a 16-bit PCM WAV.".into());
        }
        let frames = u64::from(reader.duration());
        let mut result = Self {
            reader,
            channels: spec.channels,
            rate: spec.sample_rate,
            left: 0,
            right: 0,
            input_frame: 0,
            frames,
            position: 0,
        };
        if frames > 0 {
            result.left = result.frame()?;
            result.right = if frames > 1 {
                result.frame()?
            } else {
                result.left
            };
        }
        Ok(result)
    }
    fn frame(&mut self) -> Result<i16, String> {
        let mut sum = 0_i64;
        for _ in 0..self.channels {
            sum += i64::from(
                self.reader
                    .samples::<i16>()
                    .next()
                    .ok_or("Truncated speech WAV.")?
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok((sum / i64::from(self.channels)) as i16)
    }
    fn fill(&mut self, buf: &mut Vec<i16>) -> Result<(), String> {
        buf.clear();
        while buf.len() < 960 && self.position < self.frames * u64::from(RATE) {
            let index = self.position / u64::from(RATE);
            while self.input_frame < index {
                self.left = self.right;
                self.input_frame += 1;
                self.right = if self.input_frame + 1 < self.frames {
                    self.frame()?
                } else {
                    self.left
                };
            }
            let phase = (self.position % u64::from(RATE)) as i64;
            let value = i64::from(self.left)
                + (i64::from(self.right) - i64::from(self.left)) * phase / i64::from(RATE);
            buf.push(value as i16);
            self.position += u64::from(self.rate);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Probe {
        polls: std::cell::Cell<u8>,
        attempts: std::cell::Cell<u8>,
        depth: std::cell::Cell<u8>,
        sent: std::cell::RefCell<Vec<i16>>,
    }
    impl Sink for Probe {
        fn active(&self) -> Result<bool, String> {
            self.polls.set(self.polls.get() + 1);
            Ok(self.polls.get() >= 2)
        }
        fn write(&self, samples: &[i16]) -> Result<bool, String> {
            assert!(self.polls.get() >= 2);
            self.attempts.set(self.attempts.get() + 1);
            if self.attempts.get() == 1 {
                return Ok(false);
            }
            self.sent.borrow_mut().extend_from_slice(samples);
            Ok(true)
        }
        fn queued(&self) -> Result<u32, String> {
            self.depth.set(self.depth.get() + 1);
            Ok(if self.depth.get() == 1 { 2 } else { 0 })
        }
    }
    fn test_stream() -> Mono48<io::Cursor<Vec<u8>>> {
        let mut raw = io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut raw,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in 0..1200i16 {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        Mono48::new(hound::WavReader::new(io::Cursor::new(raw.into_inner())).unwrap()).unwrap()
    }
    #[tokio::test]
    async fn waits_for_consumer_retries_backpressure_and_drains_tail() {
        let sink = Probe::default();
        pump(test_stream(), &sink, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(*sink.sent.borrow(), (0..1200i16).collect::<Vec<_>>());
        assert_eq!(sink.depth.get(), 2);
    }
    #[tokio::test]
    async fn no_consumer_cannot_receive_or_clip_the_first_packet() {
        let sink = Probe::default();
        assert!(pump(test_stream(), &sink, Duration::ZERO)
            .await
            .unwrap_err()
            .contains("No app opened"));
        assert!(sink.sent.borrow().is_empty());
        assert_eq!(sink.depth.get(), 0);
    }
    #[test]
    fn stereo_24khz_becomes_ms_sized_48khz_mono_packets() {
        let mut raw = io::Cursor::new(Vec::new());
        let mut wav = hound::WavWriter::new(
            &mut raw,
            hound::WavSpec {
                channels: 2,
                sample_rate: 24000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..600i16 {
            wav.write_sample(i * 2).unwrap();
            wav.write_sample(i * 2 + 2).unwrap();
        }
        wav.finalize().unwrap();
        let reader = hound::WavReader::new(io::Cursor::new(raw.into_inner())).unwrap();
        let mut stream = Mono48::new(reader).unwrap();
        let mut packet = Vec::new();
        stream.fill(&mut packet).unwrap();
        assert_eq!(packet.len(), 960);
        assert_eq!(&packet[..4], &[1, 2, 3, 4]);
        stream.fill(&mut packet).unwrap();
        assert_eq!(packet.len(), 240);
        assert_eq!(packet.last(), Some(&1199));
        stream.fill(&mut packet).unwrap();
        assert!(packet.is_empty());
    }
}
