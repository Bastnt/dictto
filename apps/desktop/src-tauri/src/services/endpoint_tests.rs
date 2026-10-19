use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;
use super::transcription::{TranscriptionConfig, TranscriptionEngine};
use super::llm::{LLMProcessor, ProcessingContext};

// These tests exercise the process-wide warmup/preemption coordinator.
static ACTIVITY_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn endpoint(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let length: usize = header.lines().find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap().trim().parse().unwrap();
                if bytes.len() >= end + 4 + length { break; }
            }
        }
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", reply.len(), reply).unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    });
    (base, handle)
}

#[tokio::test]
async fn transcription_uses_custom_endpoint_model_and_bearer() {
    let _activity = ACTIVITY_TEST.lock().await;
    let (base, request) = endpoint(r#"{"text":"Bonjour"}"#);
    let engine = super::transcription::openai_api::OpenAIWhisper::with_client(
        "test-api-key".into(), reqwest::Client::new(), base, "whisper-1".into());
    let result = engine.transcribe(b"wav-audio", &TranscriptionConfig {
        language: Some("fr".into()), prompt: None,
    }).await.unwrap();
    assert_eq!(result.text, "Bonjour");
    let request = request.join().unwrap();
    assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
    assert!(request.to_lowercase().contains("authorization: bearer test-api-key"));
    assert!(request.contains("whisper-1"));
    assert!(request.contains("wav-audio"));
    assert!(!request.contains("gpt-4o-mini-transcribe"));
}

#[tokio::test]
async fn cleanup_uses_custom_endpoint_and_parses_structured_text() {
    let (base, request) = endpoint(r#"{"choices":[{"message":{"content":"{\"text\":\"Bonjour.\"}"}}]}"#);
    let engine = super::llm::openai_gpt::OpenAIGPT::with_client(
        "test-api-key".into(), "cleanup-model".into(), reqwest::Client::new(), base);
    let text = engine.process("euh bonjour", &ProcessingContext {
        languages: vec!["fr".into()],
    }).await.unwrap();
    assert_eq!(text, "Bonjour.");
    let request = request.join().unwrap();
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(request.to_lowercase().contains("authorization: bearer test-api-key"));
    let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["model"], "cleanup-model");
    assert_eq!(body["response_format"]["type"], "json_schema");
}

#[tokio::test]
async fn warmup_posts_silent_wav_to_selected_model_and_discards_response() {
    let _activity = ACTIVITY_TEST.lock().await;
    let (base, request) = endpoint("{}");
    super::warmup::send(&reqwest::Client::new(), &base, "canary-1b-v2", "test-key", Some("fr")).await.unwrap();
    let request = request.join().unwrap();
    assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
    assert!(request.contains("canary-1b-v2"));
    assert!(request.contains("name=\"language\"\r\n\r\nfr"));
    assert!(request.contains("RIFF"));
    assert!(request.contains("WAVE"));
    assert!(request.to_lowercase().contains("authorization: bearer test-key"));
}

#[tokio::test]
async fn repeated_warmup_reuses_the_completed_warmup() {
    let _activity = ACTIVITY_TEST.lock().await;
    let (base, request) = endpoint("{}");
    let client = reqwest::Client::new();
    super::warmup::send(&client, &base, "local-whisper", "test-key", Some("fr")).await.unwrap();
    request.join().unwrap();
    // The endpoint has closed: a redundant POST would fail here.
    super::warmup::send(&client, &base, "local-whisper", "test-key", Some("fr")).await.unwrap();
}

#[tokio::test]
async fn real_transcription_preempts_stalled_warmup() {
    let _activity = ACTIVITY_TEST.lock().await;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (headers_sent, headers_received) = tokio::sync::oneshot::channel();
    let (release_body, wait_body) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let length: usize = header.lines().find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap().trim().parse().unwrap();
                if bytes.len() >= end + 4 + length { break; }
            }
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n").unwrap();
        headers_sent.send(()).unwrap();
        wait_body.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = stream.write_all(b"{}");
    });
    let cancelled_base = base.clone();
    let warmup = tokio::spawn(async move {
        super::warmup::send(&reqwest::Client::new(), &base, "local-whisper", "key", None).await
    });
    headers_received.await.unwrap();
    let (transcription_base, transcription_request) = endpoint(r#"{"text":"Bonjour"}"#);
    let mut transcription = tokio::spawn(async move {
        let engine = super::transcription::openai_api::OpenAIWhisper::with_client(
            "key".into(), reqwest::Client::new(), transcription_base, "local-whisper".into());
        engine.transcribe(b"wav-audio", &TranscriptionConfig { language: None, prompt: None }).await
    });
    let started = std::time::Instant::now();
    let early_result = tokio::time::timeout(Duration::from_millis(250), &mut transcription).await;
    let preempted = early_result.is_ok();
    let elapsed = started.elapsed();
    release_body.send(()).unwrap();
    warmup.await.unwrap().unwrap();
    let result = match early_result {
        Ok(result) => result,
        Err(_) => transcription.await,
    };
    assert_eq!(result.unwrap().unwrap().text, "Bonjour");
    transcription_request.join().unwrap();
    server.join().unwrap();
    assert!(preempted, "real transcription waited {:?} for a stalled warmup", elapsed);
    eprintln!("real transcription completed in {:?} while warmup body was stalled", elapsed);
    // The cancelled model was not warmed: with its endpoint closed, a retry must fail.
    assert!(super::warmup::send(&reqwest::Client::new(), &cancelled_base,
        "local-whisper", "key", None).await.is_err());
}

async fn transcription_sequence(statuses: &[u16]) -> (Result<super::transcription::TranscriptionResult, super::transcription::TranscriptionError>, Vec<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let statuses = statuses.to_vec();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for status in statuses {
            let Ok(Ok((mut stream, _))) = tokio::time::timeout(Duration::from_secs(8), listener.accept()).await else { break; };
            let mut bytes = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length: usize = header.lines().find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap().trim().parse().unwrap();
                    if bytes.len() >= end + 4 + length { break; }
                }
            }
            requests.push(String::from_utf8(bytes).unwrap());
            let body = if status == 200 { r#"{"text":"Bonjour"}"# } else { r#"{"error":"worker_unavailable"}"# };
            let reply = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(reply.as_bytes()).await.unwrap();
        }
        requests
    });
    let engine = super::transcription::openai_api::OpenAIWhisper::with_client(
        "key".into(), reqwest::Client::new(), base, "whisper-1".into());
    let result = engine.transcribe(b"original-wav-audio", &TranscriptionConfig {
        language: Some("fr".into()), prompt: Some("context".into()),
    }).await;
    (result, server.await.unwrap())
}

#[tokio::test]
async fn transcription_retries_unavailable_server_twice_and_preserves_audio() {
    let _activity = ACTIVITY_TEST.lock().await;
    let (result, requests) = transcription_sequence(&[503, 503, 200]).await;
    assert_eq!(result.expect("model should be ready on third attempt").text, "Bonjour");
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert!(request.contains("original-wav-audio"));
        assert!(request.contains("whisper-1"));
        assert!(request.contains("context"));
        assert!(request.contains("name=\"language\"\r\n\r\nfr"));
    }
}

#[tokio::test]
async fn transcription_stops_after_three_server_failures() {
    let _activity = ACTIVITY_TEST.lock().await;
    let (result, requests) = transcription_sequence(&[503, 502, 503]).await;
    assert!(result.err().unwrap().to_string().contains("503"));
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn transcription_does_not_retry_unauthorized_requests() {
    let _activity = ACTIVITY_TEST.lock().await;
    let (result, requests) = transcription_sequence(&[401]).await;
    assert!(result.err().unwrap().to_string().contains("401"));
    assert_eq!(requests.len(), 1);
}
