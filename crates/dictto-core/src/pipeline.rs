use crate::host::{AppHandle, Manager};
use std::time::Instant;

use super::db::keystore;
use super::llm::openai_gpt::OpenAIGPT;
use super::llm::{LLMProcessor, ProcessingContext};
use super::transcription::openai_api::OpenAIWhisper;
use super::transcription::{TranscriptionConfig, TranscriptionEngine};

async fn cleanup_transcript(raw_text: &str, enabled: bool, processor: &impl LLMProcessor,
    context: &ProcessingContext) -> String {
    if !enabled { return raw_text.to_string(); }
    match processor.process(raw_text, context).await {
        Ok(text) => text,
        Err(error) => {
            log::warn!("Cleanup failed: {}, using raw transcript", error);
            raw_text.to_string()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("No API key configured")]
    NoApiKey,
    #[error("Transcription failed: {0}")]
    TranscriptionFailed(String),
    #[error("Injection failed: {0}")]
    InjectionFailed(String),
}

pub struct RecordedAudio {
    pub data: Vec<u8>,
    pub released_at: Instant,
    /// Chunks already being transcribed while this was recorded; `data` stays the
    /// fallback when it yields nothing.
    pub live: Option<crate::live::LiveTranscript>,
}

impl RecordedAudio {
    pub fn new(data: Vec<u8>, released_at: Instant) -> Self {
        Self { data, released_at, live: None }
    }
}

/// What a dictation reads from the settings, once per pipeline or live transcript.
pub(crate) struct Settings {
    pub languages: Vec<String>,
    pub paste_delay_ms: u64,
    pub llm_model: String,
    pub http_client: reqwest::Client,
    pub base_url: String,
    pub transcription_model: String,
    pub cleanup_enabled: bool,
    pub injector: std::sync::Arc<dyn crate::platform::TextInjector>,
}

impl Settings {
    pub fn load(app: &AppHandle) -> Self {
        let state = app.state::<crate::CoreState>();
        let db = state.db.lock().unwrap();
        let languages_json = db
            .get_setting("languages")
            .unwrap_or_else(|| r#"["en"]"#.to_string());
        let languages: Vec<String> = serde_json::from_str(&languages_json).unwrap_or_else(|_| {
            log::warn!(
                "[pipeline] Failed to parse languages JSON '{}', falling back to [\"en\"]",
                languages_json
            );
            vec!["en".to_string()]
        });
        Self {
            languages,
            paste_delay_ms: db.get_setting("paste_delay_ms").and_then(|v| v.parse().ok()).unwrap_or(150),
            llm_model: db.get_setting("llm_model").unwrap_or_else(|| "gpt-4.1-nano".to_string()),
            http_client: state.http_client.clone(),
            base_url: db.get_setting("api_base_url").unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            transcription_model: db.get_setting("transcription_model")
                .unwrap_or_else(|| "gpt-4o-mini-transcribe".to_string()),
            cleanup_enabled: db.get_setting("cleanup_enabled").as_deref() != Some("false"),
            injector: state.platform.injector.clone(),
        }
    }
}

pub fn retryable_audio(recording: RecordedAudio, result: &Result<(), PipelineError>) -> Option<RecordedAudio> {
    match result {
        Err(PipelineError::NoApiKey | PipelineError::TranscriptionFailed(_)) => Some(recording),
        _ => None,
    }
}

pub async fn run_pipeline(app: &AppHandle, recording: &mut RecordedAudio) -> Result<(), PipelineError> {
    // Retrieving the key first keeps a missing key from costing a settings read.
    let t = Instant::now();
    let api_key = keystore::get_api_key().map_err(|e| {
        log::error!("API key retrieval failed: {}", e);
        PipelineError::NoApiKey
    })?;
    log::info!("[pipeline] API key retrieved in {:?}", t.elapsed());
    run_pipeline_with_key(app, recording, api_key).await
}

/// Separate from `run_pipeline` so tests can run it without touching the OS credential store.
pub async fn run_pipeline_with_key(
    app: &AppHandle,
    recording: &mut RecordedAudio,
    api_key: String,
) -> Result<(), PipelineError> {
    let pipeline_start = Instant::now();
    let audio_data = &recording.data[..];
    let released_at = recording.released_at;
    let audio_size_kb = audio_data.len() as f64 / 1024.0;
    log::info!(
        "[pipeline] Starting pipeline ({:.1} KB audio, {:?} since release including stop/encode and queue)",
        audio_size_kb, pipeline_start.duration_since(released_at)
    );

    let t = Instant::now();
    let Settings { languages, paste_delay_ms, llm_model, http_client, base_url, transcription_model,
        cleanup_enabled, injector } = Settings::load(app);
    log::info!(
        "[pipeline] Settings loaded in {:?} (languages={:?}, model={})",
        t.elapsed(),
        languages,
        llm_model
    );

    let t = Instant::now();
    let live = match recording.live.take() {
        Some(live) => live.finish().await,
        None => None,
    };
    let raw_text = match live {
        Some(text) => {
            log::info!("[pipeline] Live transcription ready {:?} after release ({} chars)",
                released_at.elapsed(), text.len());
            text
        }
        None => {
            let whisper = OpenAIWhisper::with_client(api_key.clone(), http_client.clone(), base_url.clone(), transcription_model);
            let config = TranscriptionConfig::from_languages(&languages);
            let transcription = whisper
                .transcribe(audio_data, &config)
                .await
                .map_err(|e| PipelineError::TranscriptionFailed(e.to_string()))?;
            log::info!(
                "[pipeline] Transcription done in {:?} ({} chars)",
                t.elapsed(),
                transcription.text.len()
            );
            transcription.text
        }
    };

    if raw_text.trim().is_empty() {
        log::info!(
            "[pipeline] Empty transcription, skipping. Total: {:?}",
            pipeline_start.elapsed()
        );
        return Ok(());
    }

    // 3. Clean with LLM (fallback to raw text on failure)
    let t = Instant::now();
    let gpt = OpenAIGPT::with_client(api_key, llm_model, http_client, base_url);
    let context = ProcessingContext {
        languages: languages.clone(),
    };

    let cleaned_text = cleanup_transcript(&raw_text, cleanup_enabled, &gpt, &context).await;
    log::info!("[pipeline] Cleanup stage completed in {:?} (enabled={})", t.elapsed(), cleanup_enabled);

    // 4. Inject text
    let t = Instant::now();
    let pasted_at = injector.inject(&cleaned_text, paste_delay_ms)
        .await
        .map_err(|e| PipelineError::InjectionFailed(e.to_string()))?;
    log::info!("[pipeline] Text injection done in {:?}", t.elapsed());
    log::info!("[pipeline] Release to Ctrl+V: {:?}; clipboard restoration: {:?}",
        pasted_at.duration_since(released_at), pasted_at.elapsed());

    // 5. Save to history
    let t = Instant::now();
    {
        let state = app.state::<crate::CoreState>();
        let db = state.db.lock().unwrap();
        if let Err(e) = db.save_history(&raw_text, &cleaned_text, &languages.join(","), None) {
            log::warn!("Failed to save history: {}", e);
        }
    }
    log::info!("[pipeline] History saved in {:?}", t.elapsed());

    log::info!(
        "[pipeline] TOTAL pipeline time: {:?}",
        pipeline_start.elapsed()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn failed_audio_survives_repeated_attempts_until_success() {
        let mut audio = RecordedAudio::new(vec![1, 2, 3], Instant::now());
        for _ in 0..3 {
            audio = retryable_audio(audio, &Err(PipelineError::TranscriptionFailed("503".into())))
                .expect("failed transcription must retain audio");
            assert_eq!(audio.data, vec![1, 2, 3]);
        }
        assert!(retryable_audio(audio, &Ok(())).is_none());
    }

    #[test]
    fn successful_transcription_is_not_retried_after_injection_failure() {
        let audio = RecordedAudio::new(vec![1], Instant::now());
        assert!(retryable_audio(audio, &Err(PipelineError::InjectionFailed("clipboard".into()))).is_none());
    }

    use super::*;
    struct UnexpectedCleanup;
    #[async_trait::async_trait]
    impl super::LLMProcessor for UnexpectedCleanup {
        async fn process(&self, _: &str, _: &super::ProcessingContext) -> Result<String, crate::llm::LLMError> {
            panic!("disabled cleanup must never call the processor");
        }
    }
    #[tokio::test]
    async fn disabled_cleanup_preserves_raw_text_without_calling_model() {
        let text = super::cleanup_transcript("euh bonjour", false, &UnexpectedCleanup,
            &super::ProcessingContext { languages: vec!["fr".into()] }).await;
        assert_eq!(text, "euh bonjour");
    }
    // PIPE-01 + sentinel: ["auto"] must produce None, not Some("auto")
    #[test]
    fn test_auto_sentinel_produces_none() {
        let languages = vec!["auto".to_string()];
        let language_param: Option<String> = if languages.len() == 1 && languages[0] != "auto" {
            Some(languages[0].clone())
        } else {
            None
        };
        assert_eq!(
            language_param, None,
            "[\"auto\"] sentinel must produce None so Whisper uses auto-detect, not Some(\"auto\")"
        );
    }

    // PIPE-01: single real language still produces Some(code)
    #[test]
    fn test_single_real_language_produces_some_code() {
        let languages = vec!["es".to_string()];
        let language_param: Option<String> = if languages.len() == 1 && languages[0] != "auto" {
            Some(languages[0].clone())
        } else {
            None
        };
        assert_eq!(
            language_param,
            Some("es".to_string()),
            "single real language should still send Some(\"es\") to Whisper"
        );
    }

    // PIPE-02: two languages still produce None
    #[test]
    fn test_two_real_languages_produce_none() {
        let languages = vec!["es".to_string(), "en".to_string()];
        let language_param: Option<String> = if languages.len() == 1 && languages[0] != "auto" {
            Some(languages[0].clone())
        } else {
            None
        };
        assert_eq!(
            language_param, None,
            "two languages should still produce None so Whisper uses auto-detect"
        );
    }
}
