use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

/// Model IDs that `/audio/transcriptions` accepts, read from the endpoint's `/models`.
pub async fn discover(client: &Client, base: &str, key: &str) -> Result<Vec<String>, String> {
    let catalog: Value = client
        .get(format!("{}/models", base.trim_end_matches('/')))
        .bearer_auth(key)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Model discovery failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Invalid model catalog: {e}"))?;
    Ok(transcription_ids(&catalog))
}

pub(crate) fn transcription_ids(catalog: &Value) -> Vec<String> {
    let entries = catalog["data"].as_array().map(Vec::as_slice).unwrap_or_default();
    // The fleet tags each route with its POST endpoint; plain OpenAI-style catalogs do not,
    // so their transcription models can only be recognised by name.
    let tagged = entries.iter().any(|e| e.get("endpoint").is_some());
    let mut ids: Vec<String> = entries
        .iter()
        .filter(|e| {
            if tagged {
                e["endpoint"].as_str().is_some_and(|p| p.ends_with("/audio/transcriptions"))
            } else {
                e["id"].as_str().is_some_and(|id| id.contains("whisper") || id.contains("transcribe"))
            }
        })
        .filter_map(|e| e["id"].as_str().map(str::to_owned))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::transcription_ids;
    use serde_json::json;

    #[test]
    fn fleet_catalog_keeps_only_transcription_routes() {
        let catalog = json!({"data": [
            {"id": "text-light", "service": "text", "endpoint": "/v1/chat/completions"},
            {"id": "canary-1b-v2", "service": "audio", "endpoint": "/v1/audio/transcriptions"},
            {"id": "audio-canary-1b-v2", "service": "audio", "endpoint": "/v1/audio/transcriptions"},
            {"id": "qwen3-tts", "service": "speech", "endpoint": "/v1/audio/speech"},
            {"id": "whisper-in-name-only", "service": "text", "endpoint": "/v1/chat/completions"}
        ]});
        assert_eq!(transcription_ids(&catalog), ["audio-canary-1b-v2", "canary-1b-v2"]);
    }

    #[test]
    fn untagged_catalog_falls_back_to_model_names() {
        let catalog = json!({"data": [
            {"id": "gpt-4.1-nano"}, {"id": "whisper-1"}, {"id": "gpt-4o-mini-transcribe"}, {"id": "tts-1"}
        ]});
        assert_eq!(transcription_ids(&catalog), ["gpt-4o-mini-transcribe", "whisper-1"]);
        assert!(transcription_ids(&json!({})).is_empty());
    }
}
