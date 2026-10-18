pub mod openai_api;

use async_trait::async_trait;

pub struct TranscriptionConfig {
    pub language: Option<String>,
    pub prompt: Option<String>,
}

impl TranscriptionConfig {
    pub fn from_languages(languages: &[String]) -> Self {
        Self {
            language: if languages.len() == 1 && languages[0] != "auto" {
                Some(languages[0].clone())
            } else { None },
            prompt: None,
        }
    }

    pub fn validate(&self, model: &str) -> Result<(), TranscriptionError> {
        if model == "canary-1b-v2" && self.language.as_deref().is_none_or(|s| s.is_empty() || s == "auto") {
            return Err(TranscriptionError::ApiError(
                "Canary does not support language auto-detection. Select one language in Settings > General > Languages.".into()));
        }
        Ok(())
    }
}

pub fn friendly_error(message: &str) -> &'static str {
    let lower = message.to_lowercase();
    if lower.contains("auto-detection") || lower.contains("explicit supported language") {
        "Canary does not support auto-detection. Select one language in Settings > General > Languages."
    } else if lower.contains("unknown_preset") {
        "Transcription model not found. Check Settings > API."
    } else if ["worker_local_use", "gpu_occupied", "worker_preempted"].iter().any(|s| lower.contains(s)) {
        "Transcription server GPU is busy. Try again shortly."
    } else if lower.contains("worker_unavailable") || lower.contains("worker_maintenance") {
        "Transcription server is unavailable. Try again shortly."
    } else if lower.contains("no api key") || lower.contains("401") || lower.contains("unauthorized") {
        "Check the API key in Settings > API."
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "Transcription timed out. Try again."
    } else if lower.contains("http error") {
        "Cannot reach the transcription server. Check Settings > API and the network."
    } else {
        "Dictation failed. Check API settings and the application log."
    }
}

pub struct TranscriptionResult {
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TranscriptionError {
    #[error("HTTP error: {0}")]
    HttpError(String),
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Timeout")]
    Timeout,
}

#[async_trait]
pub trait TranscriptionEngine: Send + Sync {
    async fn transcribe(
        &self,
        audio_data: &[u8],
        config: &TranscriptionConfig,
    ) -> Result<TranscriptionResult, TranscriptionError>;
}
