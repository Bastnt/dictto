use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use std::time::Duration;
use super::{TranscriptionConfig, TranscriptionEngine, TranscriptionError, TranscriptionResult};

pub struct OpenAIWhisper {
    base_url: String,
    model: String,
    api_key: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct WhisperResponse {
    text: String,
}

impl OpenAIWhisper {
    pub fn with_client(api_key: String, client: reqwest::Client, base_url: String, model: String) -> Self {
        Self { api_key, client, base_url, model }
    }
}

const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ATTEMPTS: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_selection_is_preserved_and_only_canary_rejects_auto() {
        for languages in [vec![], vec!["auto".into()], vec!["fr".into(), "en".into()]] {
            let config = TranscriptionConfig::from_languages(&languages);
            assert!(config.validate("canary-1b-v2").is_err());
            assert!(config.validate("whisper-1").is_ok());
        }
        let languages = vec!["fr".into()];
        let config = TranscriptionConfig::from_languages(&languages);
        assert!(config.validate("canary-1b-v2").is_ok());
        assert_eq!(config.language.as_deref(), Some("fr"));
        assert_eq!(languages, ["fr"]);
    }

    #[tokio::test]
    async fn warmup_rejects_auto_before_uploading() {
        let error = crate::services::warmup::send(&reqwest::Client::new(),
            "http://127.0.0.1:1/v1", "canary-1b-v2", "key", None).await.unwrap_err();
        assert!(error.to_string().contains("auto-detection"));
    }

    #[tokio::test]
    async fn canary_rejects_auto_before_uploading_even_empty_audio() {
        let engine = OpenAIWhisper::with_client("key".into(), reqwest::Client::new(),
            "http://127.0.0.1:1/v1".into(), "canary-1b-v2".into());
        let error = engine.transcribe(&[], &TranscriptionConfig { language: None, prompt: None })
            .await.err().expect("auto-detection must be rejected");
        assert!(error.to_string().contains("auto-detection"), "{error}");
        assert!(error.to_string().contains("Languages"), "{error}");
    }
}

#[async_trait]
impl TranscriptionEngine for OpenAIWhisper {
    async fn transcribe(
        &self,
        audio_data: &[u8],
        config: &TranscriptionConfig,
    ) -> Result<TranscriptionResult, TranscriptionError> {
        config.validate(&self.model)?;
        crate::services::warmup::prioritize_transcription();
        let mut activity = crate::services::warmup::ACTIVITY.lock().await;
        let mut last_error = TranscriptionError::Timeout;

        for attempt in 1..=MAX_ATTEMPTS {
            let file_part = multipart::Part::bytes(audio_data.to_vec())
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| TranscriptionError::HttpError(e.to_string()))?;

            let mut form = multipart::Form::new()
                .text("model", self.model.clone())
                .part("file", file_part);

            if let Some(ref lang) = config.language {
                form = form.text("language", lang.clone());
            }

            if let Some(ref prompt) = config.prompt {
                form = form.text("prompt", prompt.clone());
            }

            let response = self
                .client
                .post(format!("{}/audio/transcriptions", self.base_url.trim_end_matches('/')))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .multipart(form)
                .timeout(TRANSCRIPTION_TIMEOUT)
                .send()
                .await;

            match response {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        return Err(TranscriptionError::ApiError(format!(
                            "Status {}: {}",
                            status, body
                        )));
                    }

                    let whisper_response: WhisperResponse = resp
                        .json()
                        .await
                        .map_err(|e| TranscriptionError::ApiError(e.to_string()))?;

                    activity.completed(&self.base_url, &self.model);
                    return Ok(TranscriptionResult {
                        text: whisper_response.text,
                    });
                }
                Err(e) if e.is_timeout() && attempt < MAX_ATTEMPTS => {
                    log::debug!(
                        "[transcription] Attempt {}/{} timed out, retrying in {}s...",
                        attempt,
                        MAX_ATTEMPTS,
                        RETRY_DELAY.as_secs()
                    );
                    last_error = TranscriptionError::Timeout;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                Err(e) if e.is_timeout() => {
                    log::debug!(
                        "[transcription] Attempt {}/{} timed out, no retries left",
                        attempt, MAX_ATTEMPTS
                    );
                    return Err(TranscriptionError::Timeout);
                }
                Err(e) => {
                    return Err(TranscriptionError::HttpError(e.to_string()));
                }
            }
        }

        Err(last_error)
    }
}
