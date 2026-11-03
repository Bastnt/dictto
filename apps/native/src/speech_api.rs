use reqwest::{multipart, Client};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Voice {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Capabilities {
    pub modes: BTreeMap<String, Vec<String>>,
    pub voice_fields: BTreeMap<String, Vec<String>>,
    pub voices: Vec<Voice>,
    pub engine_options: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpeechModel {
    pub id: String,
    pub name: String,
    pub capabilities: Capabilities,
}

pub struct SpeechRequest<'a> {
    pub text: &'a str,
    pub language: &'a str,
    pub voice: Option<i64>,
    pub instruction: &'a str,
    pub options: &'a str,
    pub sample: Option<Vec<u8>>,
}

pub async fn discover(client: &Client, base: &str, key: &str) -> Result<Vec<SpeechModel>, String> {
    #[derive(Deserialize)]
    struct Catalog {
        data: Vec<Value>,
    }
    let catalog: Catalog = client
        .get(format!("{}/models", base.trim_end_matches('/')))
        .bearer_auth(key)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Speech discovery failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Invalid fleet model catalog: {e}"))?;
    catalog
        .data
        .into_iter()
        .filter(|entry| entry["service"] == "speech")
        .map(|entry| {
            serde_json::from_value(entry).map_err(|e| format!("Invalid speech model: {e}"))
        })
        .collect()
}

fn request_fields(
    model: &SpeechModel,
    input: &SpeechRequest,
) -> Result<Map<String, Value>, String> {
    let caps = &model.capabilities;
    if input.text.trim().is_empty() || input.text.chars().count() > 4096 {
        return Err("Speech input must contain 1 to 4096 characters.".into());
    }
    let mut fields = Map::from_iter([
        ("model".into(), json!(model.id)),
        ("input".into(), json!(input.text)),
    ]);
    if !input.language.trim().is_empty() {
        fields.insert("language".into(), json!(input.language));
    }
    let instruction = input.instruction.trim();
    let mode = if input.sample.is_some() {
        "clone"
    } else if caps.modes.contains_key("speaker") && input.voice.is_some() {
        let voice = input.voice.unwrap();
        if !caps.voices.iter().any(|entry| entry.id == voice) {
            return Err(format!(
                "Speaker {voice} is unavailable for {}.",
                model.name
            ));
        }
        fields.insert("voice".into(), json!(voice));
        "speaker"
    } else if !instruction.is_empty() {
        "design"
    } else {
        "auto"
    };
    if !caps.modes.contains_key(mode) {
        return Err(format!(
            "{} does not advertise {mode} speech. Check the selected voice and instruction.",
            model.name
        ));
    }
    if !instruction.is_empty()
        && caps
            .voice_fields
            .get(mode)
            .is_some_and(|v| v.iter().any(|f| f == "voice_instruction"))
    {
        if instruction.chars().count() > 1024 {
            return Err("Voice instruction exceeds 1024 characters.".into());
        }
        fields.insert("voice_instruction".into(), json!(instruction));
    }
    if !input.options.trim().is_empty() {
        let options: Map<String, Value> = serde_json::from_str(input.options)
            .map_err(|e| format!("Engine options must be a JSON object: {e}"))?;
        for (field, value) in &options {
            let valid = match caps.engine_options.get(field).map(String::as_str) {
                Some("integer") => value.is_i64() || value.is_u64(),
                Some("number") => value.is_number(),
                Some("boolean") => value.is_boolean(),
                _ => false,
            };
            if !valid {
                return Err(format!(
                    "Unsupported option or value type for {field} on {}.",
                    model.name
                ));
            }
        }
        if !options.is_empty() {
            fields.insert(
                "engine_options".into(),
                json!({(model.id.as_str()):options}),
            );
        }
    }
    Ok(fields)
}

pub async fn synthesize(
    client: &Client,
    base: &str,
    key: &str,
    model: &SpeechModel,
    input: SpeechRequest<'_>,
) -> Result<Vec<u8>, String> {
    let fields = request_fields(model, &input)?;
    let builder = client
        .post(format!("{}/audio/speech", base.trim_end_matches('/')))
        .bearer_auth(key)
        .timeout(Duration::from_secs(300));
    let request = if let Some(wav) = input.sample {
        let mut form = multipart::Form::new();
        for (key, value) in fields {
            let text = match value {
                Value::String(s) => s,
                other => other.to_string(),
            };
            form = form.text(key, text);
        }
        builder.multipart(
            form.part(
                "reference_audio",
                multipart::Part::bytes(wav)
                    .file_name("sample.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| e.to_string())?,
            ),
        )
    } else {
        builder.json(&fields)
    };
    let response = request
        .send()
        .await
        .map_err(|e| format!("Speech request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "The fleet rejected speech synthesis ({}).",
            response.status()
        ));
    }
    let audio = response.bytes().await.map_err(|e| e.to_string())?;
    let reader = hound::WavReader::new(std::io::Cursor::new(&audio))
        .map_err(|e| format!("Invalid speech WAV: {e}"))?;
    if reader.spec().sample_format != hound::SampleFormat::Int
        || reader.spec().bits_per_sample != 16
        || reader.duration() == 0
    {
        return Err("Fleet returned an empty or unsupported speech WAV.".into());
    }
    Ok(audio.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> SpeechModel {
        serde_json::from_value(json!({"id":"test", "name":"Test","capabilities":{
            "modes":{"speaker":["w"],"clone":["w"],"design":["w"]},
            "voice_fields":{"clone":["reference_audio"],"speaker":["voice","voice_instruction"],"design":["voice_instruction"]},
            "voices":[{"id":12,"name":"Speaker"}],"engine_options":{"speed":"number"}
        }})).unwrap()
    }

    fn request() -> SpeechRequest<'static> {
        SpeechRequest {
            text: "read [sigh] literally",
            language: "",
            voice: Some(12),
            instruction: "quiet",
            options: r#"{"speed":1.5}"#,
            sample: None,
        }
    }

    #[test]
    fn speaker_uses_discovered_id_and_engine_namespace() {
        let fields = request_fields(&model(), &request()).unwrap();
        assert_eq!(fields["voice"], 12);
        assert_eq!(fields["voice_instruction"], "quiet");
        assert_eq!(fields["engine_options"], json!({"test":{"speed":1.5}}));
    }

    #[test]
    fn clone_ignores_speaker_and_instruction() {
        let fields = request_fields(
            &model(),
            &SpeechRequest {
                sample: Some(vec![]),
                ..request()
            },
        )
        .unwrap();
        assert!(!fields.contains_key("voice"));
        assert!(!fields.contains_key("voice_instruction"));
    }

    #[test]
    fn design_only_engine_ignores_saved_speaker() {
        let mut model = model();
        model.capabilities.modes.remove("speaker");
        let fields = request_fields(&model, &request()).unwrap();
        assert!(!fields.contains_key("voice"));
        assert_eq!(fields["voice_instruction"], "quiet");
        assert_eq!(fields["input"], "read [sigh] literally");
    }

    #[test]
    fn rejects_unknown_speaker_and_ill_typed_advanced_options() {
        assert!(request_fields(
            &model(),
            &SpeechRequest {
                voice: Some(0),
                ..request()
            }
        )
        .is_err());
        assert!(request_fields(
            &model(),
            &SpeechRequest {
                options: r#"{"speed":true}"#,
                ..request()
            }
        )
        .is_err());
    }
    #[tokio::test]
    async fn clone_posts_authenticated_multipart_and_returns_pcm() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let mut wav = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(7_i16).unwrap();
        writer.finalize().unwrap();
        let wav = wav.into_inner();
        let expected = wav.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0; 2048];
            let headers_end = loop {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(index) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&buffer[..headers_end]).to_ascii_lowercase();
            assert!(headers.starts_with("post /v1/audio/speech "));
            assert!(headers.contains("authorization: bearer test-key\r\n"));
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            while buffer.len() < headers_end + length {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                buffer.extend_from_slice(&chunk[..n]);
            }
            let body = &buffer[headers_end..];
            let text = String::from_utf8_lossy(body);
            assert!(body.windows(expected.len()).any(|w| w == expected));
            assert!(text.contains("name=\"reference_audio\""));
            assert!(text.contains("name=\"engine_options\"\r\n\r\n{\"test\":{\"speed\":1.5}}"));
            assert!(!text.contains("name=\"voice_instruction\""));
            assert!(!text.contains("name=\"voice\""));
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                        expected.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&expected).await.unwrap();
        });
        let response = synthesize(
            &Client::new(),
            &format!("http://{addr}/v1/"),
            "test-key",
            &model(),
            SpeechRequest {
                sample: Some(wav.clone()),
                ..request()
            },
        )
        .await;
        server.await.unwrap();
        assert_eq!(response.unwrap(), wav);
    }
}
