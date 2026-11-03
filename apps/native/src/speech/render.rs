use crate::speech_api::{self, SpeechModel, SpeechRequest};
use crate::AppState;
use async_trait::async_trait;
use ring::digest::{Context, SHA256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[async_trait]
pub(super) trait Player: Sync {
    async fn chunk(&self, audio: Vec<u8>) -> Result<(), String>;
    async fn file(&self, path: &Path) -> Result<(), String>;
}

pub(super) struct WindowsPlayer;

#[async_trait]
impl Player for WindowsPlayer {
    async fn chunk(&self, audio: Vec<u8>) -> Result<(), String> {
        super::playback::play(audio).await
    }
    async fn file(&self, path: &Path) -> Result<(), String> {
        super::playback::play_file(path).await
    }
}

fn digest(parts: &[&[u8]]) -> String {
    use std::fmt::Write;
    let mut hash = Context::new(&SHA256);
    for part in parts {
        hash.update(&(part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    let mut text = String::with_capacity(64);
    for byte in hash.finish().as_ref() {
        write!(&mut text, "{byte:02x}").unwrap();
    }
    text
}

struct Synthesis<'a> {
    model: &'a SpeechModel,
    voice: Option<i64>,
    instruction: &'a str,
    language: &'a str,
    options: String,
    sample: Option<&'a [u8]>,
}

impl<'a> Synthesis<'a> {
    fn resolve(
        models: &'a [SpeechModel],
        settings: &'a HashMap<String, String>,
        language: &'a str,
        sample: Option<&'a [u8]>,
    ) -> Result<Self, String> {
        let get = |key: &str| settings.get(key).map(String::as_str).unwrap_or("");
        let model = if get("tts_model").is_empty() {
            speech_api::default_speech_model(models)
                .ok_or("The configured API does not advertise any speech models.")?
        } else {
            models.iter().find(|m| m.id == get("tts_model")).ok_or(
                "The saved speech model is unavailable. Choose another in Settings > Speech.",
            )?
        };
        let instruction_key = format!("tts_voice_instruction:{}", model.id);
        let voice = match settings.get(&format!("tts_voice:{}", model.id)) {
            Some(value) => value.parse().ok(),
            None if !settings.contains_key(&instruction_key) => {
                speech_api::default_speech_voice(model)
            }
            None => None,
        };
        let options: serde_json::Value =
            serde_json::from_str(get("tts_engine_options")).unwrap_or_default();
        Ok(Self {
            model,
            voice,
            instruction: get(&instruction_key),
            language,
            options: options
                .get(&model.id)
                .map(ToString::to_string)
                .unwrap_or_default(),
            sample,
        })
    }

    fn request<'b>(&'b self, text: &'b str, sample: Option<Vec<u8>>) -> SpeechRequest<'b> {
        SpeechRequest {
            text,
            language: self.language,
            voice: self.voice,
            instruction: self.instruction,
            options: &self.options,
            sample,
        }
    }
    fn cache_key(&self, endpoint: &str, text: &str) -> Result<String, String> {
        // Derive identity from the same capability-filtered fields as the wire request.
        let fields = speech_api::request_fields(
            self.model,
            &self.request("x", self.sample.map(|_| Vec::new())),
        )?;
        let fields = serde_json::to_vec(&fields).map_err(|e| e.to_string())?;
        Ok(digest(&[
            b"dictto-wav-v1",
            endpoint.as_bytes(),
            text.as_bytes(),
            &fields,
            self.sample.unwrap_or_default(),
        ]))
    }
    async fn chunk(
        &self,
        state: &AppState,
        base: &str,
        key: &str,
        text: &str,
    ) -> Result<Vec<u8>, String> {
        speech_api::synthesize(
            &state.http_client,
            base,
            key,
            self.model,
            self.request(text, self.sample.map(<[u8]>::to_vec)),
        )
        .await
    }
}

fn with_cache<T>(
    state: &AppState,
    op: impl FnOnce(&mut super::cache::Cache) -> io::Result<T>,
) -> Result<T, String> {
    let mut slot = state.speech_cache.lock().map_err(|e| e.to_string())?;
    if slot.is_none() || !state.speech_audio_dir.is_dir() {
        *slot = Some(
            super::cache::Cache::new(state.speech_audio_dir.clone())
                .map_err(|e| format!("Cannot open Speech Audio: {e}"))?,
        );
    }
    op(slot.as_mut().unwrap()).map_err(|e| format!("Speech audio cache: {e}"))
}

pub(super) async fn speak(
    state: &AppState,
    settings: &HashMap<String, String>,
    key: &str,
    text: &str,
    language: &str,
    sample: Option<&[u8]>,
    player: &impl Player,
) -> Result<(), String> {
    let base = settings
        .get("api_base_url")
        .ok_or("Set your speech API endpoint.")?;
    let endpoint = digest(&[base.trim_end_matches('/').as_bytes(), key.as_bytes()]);
    let catalog_key = format!("speech_catalog:{endpoint}");
    let saved = state.db.lock().unwrap().get_setting(&catalog_key);
    if let Some(models) = saved
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<SpeechModel>>(s).ok())
    {
        if let Ok(identity) = Synthesis::resolve(&models, settings, language, sample)
            .and_then(|plan| plan.cache_key(&endpoint, text))
        {
            if let Some(path) = with_cache(state, |c| c.get_path(&identity))? {
                return player.file(&path).await;
            }
        }
    }
    let models = speech_api::discover(&state.http_client, base, key).await?;
    state
        .db
        .lock()
        .unwrap()
        .set_setting(
            &catalog_key,
            &serde_json::to_string(&models).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    let plan = Synthesis::resolve(&models, settings, language, sample)?;
    let identity = plan.cache_key(&endpoint, text)?;
    if let Some(path) = with_cache(state, |c| c.get_path(&identity))? {
        return player.file(&path).await;
    }

    let mut chunks = super::speech_chunks(text);
    let first = chunks.next().ok_or("No text to speak.")?;
    let mut audio = plan.chunk(state, base, key, first).await?;
    let mut render = PendingWav::new(&state.speech_audio_dir, &audio)?;
    for next in chunks {
        render.append(&audio)?;
        let (_, following) =
            tokio::try_join!(player.chunk(audio), plan.chunk(state, base, key, next))?;
        audio = following;
    }
    render.append(&audio)?;
    player.chunk(audio).await?;
    render.finish()?;
    with_cache(state, |c| c.put_file(&identity, &render.path))?;
    Ok(())
}

struct PendingWav {
    path: PathBuf,
    writer: Option<hound::WavWriter<BufWriter<File>>>,
}

impl PendingWav {
    fn new(dir: &Path, wav: &[u8]) -> Result<Self, String> {
        let spec = hound::WavReader::new(wav)
            .map_err(|e| e.to_string())?
            .spec();
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = dir.join(format!(
                "dictto-tts-render-{}-{}.partial",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("Cannot create speech WAV: {e}")),
            };
            let mut pending = Self { path, writer: None };
            pending.writer =
                Some(hound::WavWriter::new(BufWriter::new(file), spec).map_err(|e| e.to_string())?);
            return Ok(pending);
        }
    }
    fn append(&mut self, wav: &[u8]) -> Result<(), String> {
        let mut reader = hound::WavReader::new(wav).map_err(|e| e.to_string())?;
        let writer = self.writer.as_mut().unwrap();
        let src = reader.spec();
        let dst = writer.spec();
        if src == dst {
            for sample in reader.samples::<i16>() {
                writer
                    .write_sample(sample.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        if src.channels == 0 || src.sample_rate == 0 || dst.channels == 0 || dst.sample_rate == 0 {
            return Err("Invalid speech WAV format.".into());
        }
        // Fleet chunks can come from different workers. Keep a single format for the published WAV.
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let frames = samples.len() / usize::from(src.channels);
        let output_frames =
            (frames as u64 * u64::from(dst.sample_rate)).div_ceil(u64::from(src.sample_rate));
        for target in 0..output_frames {
            let numerator = target * u64::from(src.sample_rate);
            let frame = (numerator / u64::from(dst.sample_rate)) as usize;
            let next = (frame + 1).min(frames - 1);
            let fraction =
                (numerator % u64::from(dst.sample_rate)) as f64 / f64::from(dst.sample_rate);
            let channel = |frame: usize, channel: u16| {
                let slice =
                    &samples[frame * usize::from(src.channels)..][..usize::from(src.channels)];
                if dst.channels == 1 {
                    slice.iter().map(|s| f64::from(*s)).sum::<f64>() / f64::from(src.channels)
                } else {
                    f64::from(slice[usize::from(channel.min(src.channels - 1))])
                }
            };
            for ch in 0..dst.channels {
                let a = channel(frame, ch);
                let b = channel(next, ch);
                writer
                    .write_sample((a + (b - a) * fraction).round() as i16)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        self.writer
            .take()
            .unwrap()
            .finalize()
            .map_err(|e| format!("Cannot finish speech WAV: {e}"))
    }
}
impl Drop for PendingWav {
    fn drop(&mut self) {
        drop(self.writer.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn wav(value: i16) -> Vec<u8> {
        let mut data = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut data, spec).unwrap();
        for _ in 0..100 {
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
        data.into_inner()
    }

    fn state(directory: &Path) -> AppState {
        let dir = directory.join("Speech Audio");
        AppState {
            sample_path: directory.join("voice-sample.wav"),
            speech_cache: Mutex::new(Some(super::super::cache::Cache::new(dir.clone()).unwrap())),
            speech_audio_dir: dir,
            audio_recorder: Mutex::new(crate::services::audio::recorder::AudioRecorder::new()),
            db: Mutex::new(crate::services::db::Database::new(directory.into()).unwrap()),
            http_client: reqwest::Client::new(),
            device_monitor: Arc::new(
                crate::services::audio::device_monitor::DeviceMonitorState::new(),
            ),
            device_monitor_shutdown: Mutex::new(None),
        }
    }

    struct Probe {
        chunks: Mutex<Vec<i16>>,
        files: Mutex<Vec<(PathBuf, Vec<i16>)>>,
        overlap: bool,
        second_request: Arc<tokio::sync::Notify>,
        started: tokio::sync::Notify,
        block: bool,
    }
    #[async_trait]
    impl Player for Probe {
        async fn chunk(&self, audio: Vec<u8>) -> Result<(), String> {
            let value = hound::WavReader::new(&audio[..])
                .unwrap()
                .samples::<i16>()
                .next()
                .unwrap()
                .unwrap();
            self.chunks.lock().unwrap().push(value);
            self.started.notify_one();
            if self.block {
                std::future::pending::<()>().await;
            }
            if self.overlap && value == 1 {
                tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    self.second_request.notified(),
                )
                .await
                .map_err(|_| "Playback blocked next request")?;
            }
            Ok(())
        }
        async fn file(&self, path: &Path) -> Result<(), String> {
            let samples = hound::WavReader::open(path)
                .unwrap()
                .samples()
                .collect::<Result<_, _>>()
                .unwrap();
            self.files.lock().unwrap().push((path.into(), samples));
            Ok(())
        }
    }
    fn probe(second_request: Arc<tokio::sync::Notify>, overlap: bool, block: bool) -> Probe {
        Probe {
            chunks: Default::default(),
            files: Default::default(),
            second_request,
            overlap,
            started: Default::default(),
            block,
        }
    }

    async fn server(
        fail_second: bool,
    ) -> (
        String,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Vec<String>>>,
        Arc<tokio::sync::Notify>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(tokio::sync::Notify::new());
        let log = calls.clone();
        let second = notify.clone();
        let task = tokio::spawn(async move {
            let mut spoken = 0;
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let Some(head) = request
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..head]).to_ascii_lowercase();
                let size = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                while request.len() < head + size {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let (mime, body) = if headers.starts_with("get /v1/models ") {
                    log.lock().unwrap().push("models".into());
                    ("application/json",br#"{"data":[{"id":"mock","name":"Mock","service":"speech","capabilities":{"modes":{"auto":["w"]},"voice_fields":{"auto":[]}}}]}"#.to_vec())
                } else {
                    let req: serde_json::Value = serde_json::from_slice(&request[head..]).unwrap();
                    log.lock()
                        .unwrap()
                        .push(req["input"].as_str().unwrap().into());
                    spoken += 1;
                    if fail_second && spoken == 2 {
                        let _ = socket.write_all(b"HTTP/1.1 503 Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        continue;
                    }
                    if spoken == 2 {
                        second.notify_one();
                    }
                    ("audio/wav", wav(spoken))
                };
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            }
        });
        (base, task, calls, notify)
    }

    #[tokio::test]
    async fn complete_read_replays_one_wav_after_restart_with_fleet_offline() {
        let dir =
            std::env::temp_dir().join(format!("dictto-offline-replay-{}", std::process::id()));
        let (base, server, calls, notify) = server(false).await;
        let settings = HashMap::from([
            ("api_base_url".into(), base),
            ("tts_model".into(), "mock".into()),
        ]);
        let text = format!("{}. {}", "Opening sentence ".repeat(11), "続".repeat(500));
        let first = probe(notify.clone(), true, false);
        speak(
            &state(&dir),
            &settings,
            "test credential",
            &text,
            "",
            None,
            &first,
        )
        .await
        .unwrap();
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1..].concat(), text);
        assert!(calls[1].chars().count() < 350);
        assert_eq!(*first.chunks.lock().unwrap(), [1, 2]);
        server.abort();
        let _ = server.await;
        let replay = probe(notify, false, false);
        let reopened = state(&dir);
        let time = std::time::Instant::now();
        speak(
            &reopened,
            &settings,
            "test credential",
            &text,
            "",
            None,
            &replay,
        )
        .await
        .unwrap();
        eprintln!(
            "Offline cache lookup to playback sink: {:?}",
            time.elapsed()
        );
        assert!(replay.chunks.lock().unwrap().is_empty());
        let files = replay.files.lock().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, [vec![1; 100], vec![2; 100]].concat());
        assert_eq!(files[0].0.parent().unwrap(), dir.join("Speech Audio"));
        drop(files);
        assert!(speak(
            &reopened,
            &settings,
            "other credential",
            &text,
            "",
            None,
            &replay
        )
        .await
        .is_err());
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelling_during_playback_leaves_no_wav_or_partial() {
        let dir = std::env::temp_dir().join(format!("dictto-cancel-render-{}", std::process::id()));
        let (base, server, _, notify) = server(false).await;
        let settings = HashMap::from([
            ("api_base_url".into(), base),
            ("tts_model".into(), "mock".into()),
        ]);
        let state = state(&dir);
        let player = probe(notify, false, true);
        {
            let mut running = Box::pin(speak(
                &state,
                &settings,
                "test credential",
                "cancel before publication",
                "",
                None,
                &player,
            ));
            tokio::select! {r=&mut running=>panic!("unexpected completion {r:?}"),_=player.started.notified()=>{}}
        }
        assert_eq!(
            std::fs::read_dir(&state.speech_audio_dir).unwrap().count(),
            0
        );
        server.abort();
        let _ = server.await;
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn second_chunk_failure_keeps_no_partial_result() {
        let dir = std::env::temp_dir().join(format!("dictto-failed-render-{}", std::process::id()));
        let (base, server, _, notify) = server(true).await;
        let settings = HashMap::from([
            ("api_base_url".into(), base),
            ("tts_model".into(), "mock".into()),
        ]);
        let state = state(&dir);
        let player = probe(notify, false, false);
        let text = format!("This is the first sentence. {}", "x".repeat(500));
        assert!(speak(
            &state,
            &settings,
            "test credential",
            &text,
            "",
            None,
            &player
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read_dir(&state.speech_audio_dir).unwrap().count(),
            0
        );
        server.abort();
        let _ = server.await;
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn key_uses_wire_fields_reference_content_and_authenticated_endpoint() {
        let models:Vec<SpeechModel>=serde_json::from_str(r#"[{"id":"mock","name":"Mock","capabilities":{"modes":{"speaker":["w"],"clone":["w"]},"voice_fields":{"speaker":["voice","voice_instruction"],"clone":["reference_audio"]},"voices":[{"id":1,"name":"One"},{"id":2,"name":"Two"}],"engine_options":{"speed":"number"}}}]"#).unwrap();
        let mut settings = HashMap::from([
            ("tts_model".into(), "mock".into()),
            ("tts_voice:mock".into(), "1".into()),
        ]);
        let key = |s: &HashMap<String, String>,
                   endpoint: &str,
                   text: &str,
                   language: &str,
                   sample: Option<&[u8]>| {
            Synthesis::resolve(&models, s, language, sample)
                .unwrap()
                .cache_key(endpoint, text)
                .unwrap()
        };
        let normal = key(&settings, "endpoint", "Hello", "en", None);
        let clone = key(&settings, "endpoint", "Hello", "en", Some(b"sample one"));
        assert_ne!(normal, clone);
        settings.insert("tts_voice:mock".into(), "2".into());
        settings.insert("tts_voice_instruction:mock".into(), "smile".into());
        assert_ne!(key(&settings, "endpoint", "Hello", "en", None), normal);
        assert_eq!(
            key(&settings, "endpoint", "Hello", "en", Some(b"sample one")),
            clone
        );
        assert_ne!(
            key(&settings, "endpoint", "Hello", "en", Some(b"sample two")),
            clone
        );
        for (endpoint, text, lang) in [
            ("other account", "Hello", "en"),
            ("endpoint", "Other text", "en"),
            ("endpoint", "Hello", "fr"),
        ] {
            assert_ne!(
                key(&settings, endpoint, text, lang, Some(b"sample one")),
                clone
            );
        }
        settings.insert(
            "tts_engine_options".into(),
            r#"{"mock":{"speed":1.1}}"#.into(),
        );
        assert_ne!(
            key(&settings, "endpoint", "Hello", "en", Some(b"sample one")),
            clone
        );
    }
}
