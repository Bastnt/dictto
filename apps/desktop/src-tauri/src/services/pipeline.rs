use std::time::Instant;
use tauri::Manager;

use super::db::keystore;
use super::injector;
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

pub async fn run_pipeline(
    app: &tauri::AppHandle,
    audio_data: Vec<u8>,
) -> Result<(), PipelineError> {
    let pipeline_start = Instant::now();
    let audio_size_kb = audio_data.len() as f64 / 1024.0;
    log::info!(
        "[pipeline] Starting pipeline ({:.1} KB audio)",
        audio_size_kb
    );

    // 1. Get API key
    let t = Instant::now();
    let api_key = keystore::get_api_key().map_err(|e| {
        log::error!("API key retrieval failed: {}", e);
        PipelineError::NoApiKey
    })?;
    log::info!("[pipeline] API key retrieved in {:?}", t.elapsed());

    // 2. Get settings + shared HTTP client
    let t = Instant::now();
    let (languages, paste_delay_ms, llm_model, http_client, base_url, transcription_model, cleanup_enabled) = {
        let state = app.state::<crate::AppState>();
        let db = state.db.lock().unwrap();

        // Read languages JSON array from DB (LANG-01)
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

        let paste_delay: u64 = db
            .get_setting("paste_delay_ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        let model = db
            .get_setting("llm_model")
            .unwrap_or_else(|| "gpt-4.1-nano".to_string());
        let base_url = db.get_setting("api_base_url")
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let transcription_model = db.get_setting("transcription_model")
            .unwrap_or_else(|| "gpt-4o-mini-transcribe".to_string());
        let client = state.http_client.clone();
        let cleanup_enabled = db.get_setting("cleanup_enabled").as_deref() != Some("false");
        (languages, paste_delay, model, client, base_url, transcription_model, cleanup_enabled)
    };
    log::info!(
        "[pipeline] Settings loaded in {:?} (languages={:?}, model={})",
        t.elapsed(),
        languages,
        llm_model
    );

    // 3. Transcribe audio
    let t = Instant::now();
    let whisper = OpenAIWhisper::with_client(api_key.clone(), http_client.clone(), base_url.clone(), transcription_model);
    let config = TranscriptionConfig {
        // PIPE-01: 1 real language -> send code; PIPE-02: 2+ OR ["auto"] sentinel -> None (auto-detect)
        language: if languages.len() == 1 && languages[0] != "auto" {
            Some(languages[0].clone())
        } else {
            None
        },
        prompt: None,
    };

    let transcription = whisper
        .transcribe(&audio_data, &config)
        .await
        .map_err(|e| PipelineError::TranscriptionFailed(e.to_string()))?;

    let raw_text = transcription.text;
    log::info!(
        "[pipeline] Transcription done in {:?} ({} chars)",
        t.elapsed(),
        raw_text.len()
    );

    if raw_text.trim().is_empty() {
        log::info!(
            "[pipeline] Empty transcription, skipping. Total: {:?}",
            pipeline_start.elapsed()
        );
        return Ok(());
    }

    // 4. Clean with LLM (fallback to raw text on failure)
    let t = Instant::now();
    let gpt = OpenAIGPT::with_client(api_key, llm_model, http_client, base_url);
    let context = ProcessingContext {
        languages: languages.clone(),
    };

    let cleaned_text = cleanup_transcript(&raw_text, cleanup_enabled, &gpt, &context).await;
    log::info!("[pipeline] Cleanup stage completed in {:?} (enabled={})", t.elapsed(), cleanup_enabled);

    // 5. Inject text
    let t = Instant::now();
    injector::inject_text(&cleaned_text, paste_delay_ms)
        .await
        .map_err(|e| PipelineError::InjectionFailed(e.to_string()))?;
    log::info!("[pipeline] Text injection done in {:?}", t.elapsed());

    // 6. Save to history
    let t = Instant::now();
    {
        let state = app.state::<crate::AppState>();
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
    struct UnexpectedCleanup;
    #[async_trait::async_trait]
    impl super::LLMProcessor for UnexpectedCleanup {
        async fn process(&self, _: &str, _: &super::ProcessingContext) -> Result<String, crate::services::llm::LLMError> {
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
