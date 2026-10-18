use crate::host as tauri;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;
use tauri::Manager;

pub(super) static ACTIVITY: tokio::sync::Mutex<WarmModels> = tokio::sync::Mutex::const_new(WarmModels { recent: Vec::new() });

static FOREGROUND: LazyLock<tokio::sync::watch::Sender<()>> = LazyLock::new(|| {
    tokio::sync::watch::channel(()).0
});

pub(super) fn prioritize_transcription() {
    FOREGROUND.send_replace(());
}

pub(super) struct WarmModels {
    recent: Vec<(String, String, std::time::Instant)>,
}

impl WarmModels {
    fn is_warm(&self, base: &str, model: &str) -> bool {
        self.recent.iter().any(|(b, m, time)| b == base.trim_end_matches('/') && m == model
            && time.elapsed() < Duration::from_secs(240))
    }

    pub(super) fn completed(&mut self, base: &str, model: &str) {
        self.recent.retain(|(b, m, time)| time.elapsed() < Duration::from_secs(240)
            && !(b == base.trim_end_matches('/') && m == model));
        self.recent.push((base.trim_end_matches('/').to_owned(), model.to_owned(), std::time::Instant::now()));
    }
}

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct Flight;
impl Drop for Flight {
    fn drop(&mut self) { IN_FLIGHT.store(false, Ordering::Release); }
}

fn silent_wav() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, hound::WavSpec {
            channels: 1, sample_rate: 16000, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        }).expect("valid WAV specification");
        for _ in 0..16000 { writer.write_sample(0i16).unwrap(); }
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}

pub(super) async fn send(client: &reqwest::Client, base: &str, model: &str, key: &str, language: Option<&str>) -> Result<(), super::transcription::TranscriptionError> {
    super::transcription::TranscriptionConfig { language: language.map(str::to_owned), prompt: None }.validate(model)?;
    let mut foreground = FOREGROUND.subscribe();
    let mut activity = ACTIVITY.lock().await;
    if foreground.has_changed().unwrap_or(true) || activity.is_warm(base, model) {
        return Ok(());
    }
    log::info!("[warmup] Loading model {} (language={:?})", model, language);
    let file = reqwest::multipart::Part::bytes(silent_wav())
        .file_name("warmup.wav").mime_str("audio/wav").unwrap();
    let mut form = reqwest::multipart::Form::new().text("model", model.to_owned()).part("file", file);
    if let Some(language) = language { form = form.text("language", language.to_owned()); }
    let request = async {
        client.post(format!("{}/audio/transcriptions", base.trim_end_matches('/')))
            .bearer_auth(key)
            .multipart(form)
            .timeout(Duration::from_secs(120)).send().await?.error_for_status()?.bytes().await?;
        Ok::<(), reqwest::Error>(())
    };
    tokio::select! {
        biased;
        // Drop the background request and its lock before real audio is submitted.
        _ = foreground.changed() => return Ok(()),
        result = request => result.map_err(|e| super::transcription::TranscriptionError::HttpError(e.to_string()))?,
    }
    activity.completed(base, model);
    log::info!("[warmup] Model {} is ready", model);
    Ok(())
}

pub fn schedule(app: &tauri::AppHandle) {
    let state = app.state::<crate::AppState>();
    let (base, model, config) = {
        let db = state.db.lock().unwrap();
        if db.get_setting("transcription_warmup").as_deref() != Some("true") { return; }
        let Some(base) = db.get_setting("api_base_url") else { return; };
        let Some(model) = db.get_setting("transcription_model") else { return; };
        let languages: Vec<String> = serde_json::from_str(&db.get_setting("languages").unwrap_or_default()).unwrap_or_default();
        (base, model, super::transcription::TranscriptionConfig::from_languages(&languages))
    };
    if IN_FLIGHT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { return; }
    let client = state.http_client.clone();
    tauri::async_runtime::spawn(async move {
        let _flight = Flight;
        let Ok(key) = super::db::keystore::get_api_key() else { return; };
        // Warmup never enters the dictation pipeline or emits user-facing events.
        if let Err(error) = send(&client, &base, &model, &key, config.language.as_deref()).await {
            log::warn!("Transcription warmup failed: {}", error);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn warmup_is_one_second_of_mono_silence() {
        let wav = super::silent_wav();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.duration(), 16000);
        assert!(reader.samples::<i16>().all(|sample| sample.unwrap() == 0));
    }
}
