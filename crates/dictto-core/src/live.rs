//! Transcribes a dictation chunk by chunk while it is still being recorded, so that only
//! the last chunk is left to transcribe when the user releases the shortcut.
use crate::audio::dsp::{encode_wav, SAMPLE_RATE};
use crate::audio::segmenter::Segmenter;
use crate::host::{self, AppHandle};
use crate::pipeline::Settings;
use crate::platform::SampleTap;
use crate::transcription::openai_api::OpenAIWhisper;
use crate::transcription::{TranscriptionConfig, TranscriptionEngine};
use std::sync::Arc;
use std::time::Instant;

/// Roughly the last sentence, which tells the model how the text so far ends.
const PROMPT_CHARS: usize = 200;

/// Dropping it stops the transcription of chunks still to come.
pub struct LiveTranscript {
    task: host::async_runtime::JoinHandle<Result<Option<String>, String>>,
}

impl Drop for LiveTranscript {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LiveTranscript {
    /// The text of every chunk, or `None` when nothing was streamed or a chunk failed, in
    /// which case the caller transcribes the whole recording.
    pub async fn finish(mut self) -> Option<String> {
        match (&mut self.task).await {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => {
                log::warn!("[live] Chunk transcription failed, falling back to the whole recording: {}", error);
                None
            }
            Err(error) => {
                log::warn!("[live] Live transcription stopped: {}", error);
                None
            }
        }
    }
}

/// The tap feeds the recorder's packets to the transcript; it ends when the recorder
/// drops the tap at stop.
pub fn start(app: &AppHandle) -> (SampleTap, LiveTranscript) {
    let (samples_tx, samples) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let tap: SampleTap = Arc::new(move |packet: &[i16]| {
        let _ = samples_tx.send(packet.to_vec());
    });
    let settings = Settings::load(app);
    let task = host::async_runtime::spawn(run(settings, samples));
    (tap, LiveTranscript { task })
}

async fn run(settings: Settings, samples: tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>)
    -> Result<Option<String>, String> {
    let api_key = crate::db::keystore::get_api_key().map_err(|e| e.to_string())?;
    run_with_key(settings, api_key, samples).await
}

/// Separate from `run` so tests can run it without touching the OS credential store.
pub(crate) async fn run_with_key(settings: Settings, api_key: String,
    mut samples: tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>) -> Result<Option<String>, String> {
    let (chunks_tx, chunks) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let segment = async move {
        let mut segmenter = Segmenter::new();
        let mut received = false;
        while let Some(packet) = samples.recv().await {
            received = true;
            for chunk in segmenter.push(&packet) {
                let _ = chunks_tx.send(chunk);
            }
        }
        if let Some(chunk) = segmenter.finish() {
            let _ = chunks_tx.send(chunk);
        }
        received
    };
    let (received, text) = tokio::join!(segment, transcribe(settings, api_key, chunks));
    // A recorder that ignores the tap streams nothing: the whole recording is the source.
    if !received {
        return Ok(None);
    }
    text
}

async fn transcribe(settings: Settings, api_key: String,
    mut chunks: tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>) -> Result<Option<String>, String> {
    let prompts = accepts_prompt(&settings.transcription_model);
    let engine = OpenAIWhisper::with_client(api_key, settings.http_client, settings.base_url,
        settings.transcription_model);
    let mut text = String::new();
    let mut count = 0;
    while let Some(chunk) = chunks.recv().await {
        count += 1;
        let t = Instant::now();
        let wav = encode_wav(&chunk).map_err(|e| e.to_string())?;
        let mut config = TranscriptionConfig::from_languages(&settings.languages);
        if prompts && !text.is_empty() {
            config.prompt = Some(tail(&text, PROMPT_CHARS).to_string());
        }
        let part = engine.transcribe(&wav, &config).await.map_err(|e| e.to_string())?.text;
        log::info!("[live] Chunk {} ({:.1}s of audio) transcribed in {:?} ({} chars)",
            count, chunk.len() as f64 / SAMPLE_RATE as f64, t.elapsed(), part.len());
        let part = part.trim();
        if !part.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(part);
        }
    }
    Ok((count > 0).then_some(text))
}

/// OpenAI and Whisper servers read the prompt as the text preceding the audio; Canary
/// has no such input, so it is left out there.
fn accepts_prompt(model: &str) -> bool {
    !model.starts_with("canary")
}

/// The end of `text`, starting at a word boundary.
fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    tail.find(' ').map_or(tail, |space| &tail[space + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_tail_starts_on_a_whole_word() {
        assert_eq!(tail("short", 200), "short");
        assert_eq!(tail("un deux trois quatre", 9), "quatre");
        assert_eq!(tail("été café hôtel", 7), "hôtel");
    }
}
