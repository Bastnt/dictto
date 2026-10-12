use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;
use super::transcription::{TranscriptionConfig, TranscriptionEngine};
use super::llm::{LLMProcessor, ProcessingContext};

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
        String::from_utf8(bytes).unwrap()
    });
    (base, handle)
}

#[tokio::test]
async fn transcription_uses_custom_endpoint_model_and_bearer() {
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
