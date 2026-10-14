use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::Manager;

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

pub(super) async fn send(client: &reqwest::Client, base: &str, model: &str, key: &str) -> Result<(), reqwest::Error> {
    let file = reqwest::multipart::Part::bytes(silent_wav())
        .file_name("warmup.wav").mime_str("audio/wav").unwrap();
    client.post(format!("{}/audio/transcriptions", base.trim_end_matches('/')))
        .bearer_auth(key)
        .multipart(reqwest::multipart::Form::new().text("model", model.to_owned()).part("file", file))
        .timeout(Duration::from_secs(120)).send().await?.error_for_status()?;
    Ok(())
}

pub fn schedule(app: &tauri::AppHandle) {
    let state = app.state::<crate::AppState>();
    let (base, model) = {
        let db = state.db.lock().unwrap();
        if db.get_setting("transcription_warmup").as_deref() != Some("true") { return; }
        let Some(base) = db.get_setting("api_base_url") else { return; };
        let Some(model) = db.get_setting("transcription_model") else { return; };
        (base, model)
    };
    if IN_FLIGHT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { return; }
    let client = state.http_client.clone();
    tauri::async_runtime::spawn(async move {
        let _flight = Flight;
        let Ok(key) = super::db::keystore::get_api_key() else { return; };
        // Warmup never enters the dictation pipeline or emits user-facing events.
        if send(&client, &base, &model, &key).await.is_err() {
            log::debug!("Transcription warmup failed; normal dictation remains available");
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
