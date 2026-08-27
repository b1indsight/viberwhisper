use crate::audio::{WavChunk, contains_audible_window};
use crate::core::config::{ApiAuth, ConfigKey, TranscriptionSection, ValidationIssue};
use crate::transcriber::TranscribeError;
use std::io::Cursor;
use std::time::Duration;
use tracing::{info, instrument, warn};

/// Two attempts plus the retry backoff stay below the 30-second session budget.
const STT_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const STT_MAX_RETRIES: u32 = 1;

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, chunk: &WavChunk) -> Result<String, TranscribeError>;
}

#[cfg(test)]
pub struct MockTranscriber;

#[cfg(test)]
impl Transcriber for MockTranscriber {
    #[instrument(name = "mock_stt", skip(self, _chunk))]
    fn transcribe(&self, _chunk: &WavChunk) -> Result<String, TranscribeError> {
        info!("Starting transcription");
        let text = "This is mock transcribed text".to_string();
        info!(result = %text, "Transcription complete");
        Ok(text)
    }
}

#[derive(Debug)]
pub struct TranscriberConfig {
    endpoint: reqwest::Url,
    auth: ApiAuth,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TranscriberMetadata {
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) language: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) temperature: f32,
}

impl TranscriberConfig {
    pub(crate) fn validate(
        endpoint: &str,
        auth: ApiAuth,
        model: &str,
        transcription: &TranscriptionSection,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        let endpoint = match reqwest::Url::parse(endpoint) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
            Ok(_) => {
                issues.push(ValidationIssue::new(
                    ConfigKey::ApiTranscriptionUrl,
                    "transcriber.url_scheme",
                    "transcription URL must use http or https",
                ));
                None
            }
            Err(error) => {
                issues.push(ValidationIssue::new(
                    ConfigKey::ApiTranscriptionUrl,
                    "transcriber.url_invalid",
                    format!("invalid transcription URL: {error}"),
                ));
                None
            }
        };
        if model.trim().is_empty() {
            issues.push(ValidationIssue::new(
                ConfigKey::ApiTranscriptionModel,
                "transcriber.model_empty",
                "transcription model cannot be empty",
            ));
        }
        match endpoint {
            Some(endpoint) if issues.is_empty() => Ok(Self {
                endpoint,
                auth,
                model: model.to_string(),
                language: transcription.language.clone(),
                prompt: transcription.prompt.clone(),
                temperature: transcription.temperature,
            }),
            _ => Err(issues),
        }
    }

    pub(crate) fn metadata(&self) -> TranscriberMetadata {
        let mut endpoint = self.endpoint.clone();
        let _ = endpoint.set_username("");
        let _ = endpoint.set_password(None);
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        TranscriberMetadata {
            endpoint: endpoint.to_string(),
            model: self.model.clone(),
            language: self.language.clone(),
            prompt: self.prompt.clone(),
            temperature: self.temperature,
        }
    }

    pub(crate) fn with_prompt(mut self, prompt: Option<String>) -> Self {
        self.prompt = prompt;
        self
    }
}

/// Generic HTTP-based transcriber compatible with OpenAI-style multipart audio endpoints.
///
/// Initialized from config via `api_key`, `transcription_api_url`, and `model`.
/// No provider name is hardcoded — the caller supplies all connection details through config.
///
/// Each call uploads one already-encoded in-memory WAV chunk. Chunk production and ordered
/// result merging belong to the recording and offline-conversion workflows.
pub struct ApiTranscriber {
    auth: ApiAuth,
    api_url: reqwest::Url,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
    /// Shared HTTP client (connection reuse + request timeout).
    client: reqwest::blocking::Client,
}

impl ApiTranscriber {
    pub fn new(config: TranscriberConfig) -> Result<Self, reqwest::Error> {
        let transcriber = Self {
            auth: config.auth,
            api_url: config.endpoint,
            model: config.model,
            language: config.language,
            prompt: config.prompt,
            temperature: config.temperature,
            client: reqwest::blocking::Client::builder()
                .timeout(STT_REQUEST_TIMEOUT)
                .build()?,
        };
        info!(
            model = %transcriber.model,
            api_url = %transcriber.api_url,
            language = transcriber.language.as_deref().unwrap_or("auto"),
            max_retries = STT_MAX_RETRIES,
            "Using API transcriber for speech recognition"
        );
        Ok(transcriber)
    }

    /// Upload one complete in-memory WAV payload and return its transcription text.
    fn upload_chunk(&self, chunk: &WavChunk) -> Result<String, TranscribeError> {
        let part = reqwest::blocking::multipart::Part::reader_with_length(
            Cursor::new(chunk.shared_bytes()),
            chunk.len() as u64,
        )
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| TranscribeError::Network(e.to_string()))?;

        let mut form = reqwest::blocking::multipart::Form::new()
            .text("model", self.model.clone())
            .text("temperature", self.temperature.to_string())
            .text("response_format", "verbose_json")
            .part("file", part);

        if let Some(lang) = &self.language {
            form = form.text("language", lang.clone());
        }
        if let Some(prompt) = &self.prompt {
            form = form.text("prompt", prompt.clone());
        }

        let mut request = self.client.post(self.api_url.clone()).multipart(form);
        if let ApiAuth::Bearer(secret) = &self.auth {
            request = request.bearer_auth(secret.expose());
        }
        let response = request
            .send()
            .map_err(|e| TranscribeError::Network(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .map_err(|e| TranscribeError::Network(e.to_string()))?;

        if !status.is_success() {
            return Err(TranscribeError::Api {
                status: status.as_u16(),
                body,
            });
        }

        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| TranscribeError::Network(format!("invalid JSON response: {e}")))?;
        let text = json["text"]
            .as_str()
            .ok_or_else(|| {
                TranscribeError::Network("text field not found in response".to_string())
            })?
            .trim()
            .to_string();

        Ok(text)
    }

    /// Return true if an HTTP status code is retryable (5xx server errors).
    fn is_retryable_status(status: u16) -> bool {
        status >= 500
    }

    /// Upload a chunk with exponential-backoff retry.
    ///
    /// Retries on: network/connection errors, HTTP 5xx.
    /// Does NOT retry: HTTP 4xx (client errors — retrying is futile).
    fn upload_chunk_with_retry(&self, chunk: &WavChunk) -> Result<String, TranscribeError> {
        self.upload_chunk_with_retry_using(chunk, std::thread::sleep)
    }

    /// Runs the retry policy with an injected wait operation so tests can avoid wall-clock delay.
    fn upload_chunk_with_retry_using(
        &self,
        chunk: &WavChunk,
        mut sleep: impl FnMut(std::time::Duration),
    ) -> Result<String, TranscribeError> {
        let mut last_error = TranscribeError::Network("upload not attempted".to_string());

        for attempt in 0..=STT_MAX_RETRIES {
            if attempt > 0 {
                let wait_secs = std::cmp::min(1u64 << (attempt - 1), 16);
                warn!(
                    attempt = attempt,
                    wait_secs = wait_secs,
                    "Retrying chunk upload"
                );
                sleep(std::time::Duration::from_secs(wait_secs));
            }

            info!(attempt = attempt, "Uploading chunk");

            match self.upload_chunk(chunk) {
                Ok(text) => return Ok(text),
                // Client errors (4xx) are not transient — retrying is futile.
                Err(e @ TranscribeError::Api { status, .. })
                    if !Self::is_retryable_status(status) =>
                {
                    return Err(e);
                }
                Err(e) => {
                    warn!(
                        attempt = attempt,
                        error = %e,
                        "Chunk upload failed"
                    );
                    last_error = e;
                }
            }
        }

        warn!(
            attempts = STT_MAX_RETRIES + 1,
            error = %last_error,
            "Chunk upload failed after all retries"
        );
        Err(last_error)
    }
}

impl Transcriber for ApiTranscriber {
    #[instrument(name = "api_stt", skip(self, chunk), fields(bytes = chunk.len()))]
    fn transcribe(&self, chunk: &WavChunk) -> Result<String, TranscribeError> {
        info!("Starting transcription");
        match contains_audible_window(chunk) {
            Ok(false) => {
                info!("Skipping effectively silent audio chunk");
                return Ok(String::new());
            }
            Ok(true) => {}
            Err(error) => {
                warn!(
                    error = %error,
                    "Could not classify audio signal; preserving upload behavior"
                );
            }
        }
        let text = self.upload_chunk_with_retry(chunk)?;
        info!(result = %text, "Transcription complete");
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunk::encode_i16_wav;
    use crate::core::config::{ApiAuth, SecretValue, TranscriptionSection};

    fn validated_config(endpoint: &str) -> TranscriberConfig {
        validated_config_with_auth(endpoint, ApiAuth::Bearer(SecretValue::new("test_key")))
    }

    fn validated_config_with_auth(endpoint: &str, auth: ApiAuth) -> TranscriberConfig {
        TranscriberConfig::validate(
            endpoint,
            auth,
            "whisper-large-v3-turbo",
            &TranscriptionSection::default(),
        )
        .unwrap()
    }

    #[test]
    fn metadata_omits_endpoint_credentials_and_query_parameters() {
        let config = validated_config(
            "https://user:password@api.example.test/v1/audio/transcriptions?token=secret#fragment",
        );

        let metadata = config.metadata();

        assert_eq!(
            metadata.endpoint,
            "https://api.example.test/v1/audio/transcriptions"
        );
        assert_eq!(metadata.model, "whisper-large-v3-turbo");
        assert_eq!(metadata.language.as_deref(), Some("zh"));
        assert_eq!(metadata.temperature, 0.0);
        assert!(metadata.prompt.is_some());
    }

    #[test]
    fn prompt_override_changes_only_the_in_memory_transcriber_prompt() {
        let config = validated_config("https://api.example.test/v1/audio/transcriptions");
        let before = config.metadata();

        let after = config
            .with_prompt(Some("candidate prompt".to_string()))
            .metadata();

        assert_eq!(after.endpoint, before.endpoint);
        assert_eq!(after.model, before.model);
        assert_eq!(after.language, before.language);
        assert_eq!(after.temperature, before.temperature);
        assert_eq!(after.prompt.as_deref(), Some("candidate prompt"));
    }

    // --- structured error tests against a local HTTP stub ---

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// Minimal HTTP server that answers every connection with a fixed response
    /// and counts how many requests it served.
    fn spawn_http_stub(status_line: &'static str, body: &'static str) -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                // Drain the request (multipart body arrives in several reads);
                // a short read timeout marks the end of the client's send.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(100)));
                let mut buf = [0u8; 16384];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                let response = format!(
                    "{status_line}\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (port, requests)
    }

    fn spawn_request_stub() -> (u16, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(100)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 16_384];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => request.extend_from_slice(&buffer[..size]),
                }
            }
            sender.send(request).unwrap();
            let body = r#"{"text":"ok"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (port, receiver)
    }

    fn transcriber_for_port(port: u16) -> ApiTranscriber {
        let config = validated_config(&format!("http://127.0.0.1:{port}/v1/audio/transcriptions"));
        ApiTranscriber::new(config).unwrap()
    }

    fn test_chunk() -> WavChunk {
        WavChunk::from_encoded_bytes(b"fake wav bytes".to_vec())
    }

    fn silent_chunk() -> WavChunk {
        encode_i16_wav(&vec![0; 3_200], 16_000).unwrap()
    }

    fn audible_chunk() -> WavChunk {
        encode_i16_wav(&vec![200; 1_600], 16_000).unwrap()
    }

    #[test]
    fn silent_chunk_returns_empty_without_an_http_request() {
        let (port, requests) = spawn_http_stub("HTTP/1.1 200 OK", "{\"text\":\"hallucination\"}");
        let transcriber = transcriber_for_port(port);

        let result = transcriber.transcribe(&silent_chunk());

        assert_eq!(result.unwrap(), "");
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn audible_chunk_still_uses_the_normal_upload_path() {
        let (port, requests) = spawn_http_stub("HTTP/1.1 200 OK", "{\"text\":\"spoken\"}");
        let transcriber = transcriber_for_port(port);

        let result = transcriber.transcribe(&audible_chunk());

        assert_eq!(result.unwrap(), "spoken");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn signal_classification_failure_preserves_the_upload_path() {
        let (port, requests) = spawn_http_stub("HTTP/1.1 200 OK", "{\"text\":\"fallback\"}");
        let transcriber = transcriber_for_port(port);

        let result = transcriber.transcribe(&test_chunk());

        assert_eq!(result.unwrap(), "fallback");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_upload_success_returns_trimmed_text() {
        let (port, _requests) = spawn_http_stub("HTTP/1.1 200 OK", "{\"text\": \" hello \"}");
        let t = transcriber_for_port(port);
        let chunk = test_chunk();

        let result = t.upload_chunk_with_retry(&chunk);

        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn authorization_header_matches_typed_auth_mode() {
        for (auth, expected_header) in [
            (ApiAuth::None, None),
            (
                ApiAuth::Bearer(SecretValue::new("header-token")),
                Some("authorization: bearer header-token"),
            ),
        ] {
            let (port, request) = spawn_request_stub();
            let config = validated_config_with_auth(
                &format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
                auth,
            );
            let transcriber = ApiTranscriber::new(config).unwrap();
            let chunk = test_chunk();

            assert_eq!(transcriber.upload_chunk(&chunk).unwrap(), "ok");
            let request = request.recv().unwrap();
            let headers = String::from_utf8_lossy(&request)
                .split("\r\n\r\n")
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            match expected_header {
                Some(expected) => assert!(headers.contains(expected)),
                None => assert!(!headers.contains("authorization:")),
            }
        }
    }

    #[test]
    fn multipart_upload_contains_the_shared_chunk_bytes() {
        let (port, request) = spawn_request_stub();
        let transcriber = transcriber_for_port(port);
        let chunk = test_chunk();

        assert_eq!(transcriber.upload_chunk(&chunk).unwrap(), "ok");
        let request = request.recv().unwrap();
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.contains("filename=\"audio.wav\""));
        assert!(
            request
                .windows(chunk.bytes().len())
                .any(|window| window == chunk.bytes())
        );
    }

    #[test]
    fn test_client_error_is_structured_and_not_retried() {
        let (port, requests) =
            spawn_http_stub("HTTP/1.1 400 Bad Request", "{\"error\":\"bad model\"}");
        let t = transcriber_for_port(port);
        let chunk = test_chunk();
        let mut waits = Vec::new();

        let result = t.upload_chunk_with_retry_using(&chunk, |duration| waits.push(duration));

        match result {
            Err(TranscribeError::Api { status: 400, body }) => {
                assert!(body.contains("bad model"));
            }
            other => panic!("expected structured 400 error, got {:?}", other),
        }
        // No retry: a single request, no exponential-backoff sleeps.
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(waits.is_empty());
    }

    #[test]
    fn test_server_error_is_retried_and_kept_structured() {
        let (port, requests) =
            spawn_http_stub("HTTP/1.1 503 Service Unavailable", "{\"error\":\"busy\"}");
        let t = transcriber_for_port(port);
        let chunk = test_chunk();
        let mut waits = Vec::new();

        let result = t.upload_chunk_with_retry_using(&chunk, |duration| waits.push(duration));

        assert!(matches!(
            result,
            Err(TranscribeError::Api { status: 503, .. })
        ));
        // Initial attempt + one retry.
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(waits, vec![std::time::Duration::from_secs(1)]);
    }
}
