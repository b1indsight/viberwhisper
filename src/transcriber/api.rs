use crate::audio::split_wav;
use crate::core::config::AppConfig;
use crate::text::merge_texts;
use crate::transcriber::TranscribeError;
use std::time::Duration;
use tracing::{info, instrument, warn};

/// Total per-request timeout for one chunk upload (connect + send + response).
/// Without it, a hung request would pin the orchestrator worker thread forever;
/// the convergence timeout only stops the caller from waiting, not the worker.
const STT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, wav_path: &str) -> Result<String, TranscribeError>;
}

pub struct MockTranscriber;

impl Transcriber for MockTranscriber {
    #[instrument(name = "mock_stt", skip(self), fields(path = %wav_path))]
    fn transcribe(&self, wav_path: &str) -> Result<String, TranscribeError> {
        info!("Starting transcription");
        let text = "This is mock transcribed text".to_string();
        info!(result = %text, "Transcription complete");
        Ok(text)
    }
}

/// Generic HTTP-based transcriber compatible with OpenAI-style multipart audio endpoints.
///
/// Initialized from config via `api_key`, `transcription_api_url`, and `model`.
/// No provider name is hardcoded — the caller supplies all connection details through config.
///
/// For audio files that exceed `max_chunk_size_bytes` or `max_chunk_duration_secs`, the
/// transcriber will automatically split the file into smaller chunks, upload each chunk
/// individually (with exponential-backoff retry on transient errors), and merge the results.
pub struct ApiTranscriber {
    api_key: String,
    api_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
    /// Maximum duration per chunk in seconds. 0 = no duration limit.
    max_chunk_duration_secs: u32,
    /// Maximum byte size per chunk (including WAV header). 0 = no size limit.
    max_chunk_size_bytes: u64,
    /// Maximum retry attempts per chunk on transient errors (5xx / network).
    max_retries: u32,
    /// Shared HTTP client (connection reuse + request timeout).
    client: reqwest::blocking::Client,
}

impl ApiTranscriber {
    pub fn from_config(config: &AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let api_key = config
            .api_key
            .clone()
            .ok_or("api_key not configured (set api_key in config.json or GROQ_API_KEY env var)")?;
        Ok(Self {
            api_key,
            api_url: config.transcription_api_url.clone(),
            model: config.model.clone(),
            language: config.language.clone(),
            prompt: config.prompt.clone(),
            temperature: config.temperature,
            max_chunk_duration_secs: config.max_chunk_duration_secs,
            max_chunk_size_bytes: config.max_chunk_size_bytes,
            max_retries: config.max_retries,
            client: reqwest::blocking::Client::builder()
                .timeout(STT_REQUEST_TIMEOUT)
                .build()?,
        })
    }

    /// Upload a single WAV file and return its transcription text.
    fn upload_file(&self, wav_path: &str) -> Result<String, TranscribeError> {
        let file_bytes = std::fs::read(wav_path)
            .map_err(|e| TranscribeError::Network(format!("failed to read {wav_path}: {e}")))?;
        let file_name = std::path::Path::new(wav_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.wav")
            .to_string();

        let part = reqwest::blocking::multipart::Part::bytes(file_bytes)
            .file_name(file_name)
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

        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
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
    fn upload_file_with_retry(
        &self,
        wav_path: &str,
        chunk_index: usize,
        total_chunks: usize,
    ) -> Result<String, TranscribeError> {
        let mut last_error = TranscribeError::Network("upload not attempted".to_string());

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let wait_secs = std::cmp::min(1u64 << (attempt - 1), 16);
                warn!(
                    chunk = chunk_index + 1,
                    total = total_chunks,
                    attempt = attempt,
                    wait_secs = wait_secs,
                    "Retrying chunk upload"
                );
                std::thread::sleep(std::time::Duration::from_secs(wait_secs));
            }

            info!(
                chunk = chunk_index + 1,
                total = total_chunks,
                attempt = attempt,
                "Uploading chunk"
            );

            match self.upload_file(wav_path) {
                Ok(text) => return Ok(text),
                // Client errors (4xx) are not transient — retrying is futile.
                Err(e @ TranscribeError::Api { status, .. })
                    if !Self::is_retryable_status(status) =>
                {
                    return Err(e);
                }
                Err(e) => {
                    warn!(
                        chunk = chunk_index + 1,
                        total = total_chunks,
                        attempt = attempt,
                        error = %e,
                        "Chunk upload failed"
                    );
                    last_error = e;
                }
            }
        }

        warn!(
            chunk = chunk_index + 1,
            total = total_chunks,
            attempts = self.max_retries + 1,
            error = %last_error,
            "Chunk upload failed after all retries"
        );
        Err(last_error)
    }
}

impl Transcriber for ApiTranscriber {
    #[instrument(name = "api_stt", skip(self), fields(path = %wav_path))]
    fn transcribe(&self, wav_path: &str) -> Result<String, TranscribeError> {
        info!("Starting transcription");

        let chunks = split_wav(
            wav_path,
            self.max_chunk_duration_secs,
            self.max_chunk_size_bytes,
        )
        .map_err(|e| TranscribeError::Network(format!("failed to split {wav_path}: {e}")))?;

        if chunks.is_empty() {
            // File fits within limits — use single-shot upload path (no splitting overhead).
            let text = self.upload_file_with_retry(wav_path, 0, 1)?;
            info!(result = %text, "Transcription complete");
            return Ok(text);
        }

        let total = chunks.len();
        info!(chunks = total, "Audio split into chunks for transcription");

        let mut texts: Vec<String> = Vec::with_capacity(total);
        for chunk in &chunks {
            let text = self.upload_file_with_retry(chunk.path_str(), chunk.index, total)?;
            texts.push(text);
        }

        let result = merge_texts(&texts, self.language.as_deref());
        info!(result = %result, chunks = total, "Transcription complete (merged)");
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    #[test]
    fn test_mock_transcriber_returns_text() {
        let t = MockTranscriber;
        let result = t.transcribe("fake.wav");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_api_transcriber_from_config_no_key_fails() {
        let config = AppConfig::default(); // no api_key
        let result = ApiTranscriber::from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_api_transcriber_from_config_with_key() {
        let config = AppConfig {
            api_key: Some("test_key".to_string()),
            ..Default::default()
        };
        let result = ApiTranscriber::from_config(&config);
        assert!(result.is_ok());
        let t = result.unwrap();
        assert_eq!(t.api_key, "test_key");
        assert_eq!(
            t.api_url,
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(t.model, "whisper-large-v3-turbo");
        assert_eq!(t.max_chunk_duration_secs, 30);
        assert_eq!(t.max_chunk_size_bytes, 23 * 1024 * 1024);
        assert_eq!(t.max_retries, 3);
    }

    #[test]
    fn test_api_transcriber_custom_url() {
        let config = AppConfig {
            api_key: Some("key".to_string()),
            transcription_api_url: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            ..Default::default()
        };
        let t = ApiTranscriber::from_config(&config).unwrap();
        assert_eq!(t.api_url, "https://api.openai.com/v1/audio/transcriptions");
    }

    #[test]
    fn test_api_transcriber_chunk_config_from_config() {
        let config = AppConfig {
            api_key: Some("key".to_string()),
            max_chunk_duration_secs: 60,
            max_chunk_size_bytes: 10_000_000,
            max_retries: 5,
            ..Default::default()
        };
        let t = ApiTranscriber::from_config(&config).unwrap();
        assert_eq!(t.max_chunk_duration_secs, 60);
        assert_eq!(t.max_chunk_size_bytes, 10_000_000);
        assert_eq!(t.max_retries, 5);
    }

    #[test]
    fn test_is_retryable_status() {
        assert!(ApiTranscriber::is_retryable_status(500));
        assert!(ApiTranscriber::is_retryable_status(503));
        assert!(!ApiTranscriber::is_retryable_status(400));
        assert!(!ApiTranscriber::is_retryable_status(404));
        assert!(!ApiTranscriber::is_retryable_status(429));
    }

    // --- structured error tests against a local HTTP stub ---

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn transcriber_for_port(port: u16, max_retries: u32) -> ApiTranscriber {
        let config = AppConfig {
            api_key: Some("test_key".to_string()),
            transcription_api_url: format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
            max_retries,
            ..Default::default()
        };
        ApiTranscriber::from_config(&config).unwrap()
    }

    fn temp_upload_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "viberwhisper-api-test-{}-{name}.wav",
            std::process::id()
        ));
        std::fs::write(&path, b"fake wav bytes").unwrap();
        path
    }

    #[test]
    fn test_upload_success_returns_trimmed_text() {
        let (port, _requests) = spawn_http_stub("HTTP/1.1 200 OK", "{\"text\": \" hello \"}");
        let t = transcriber_for_port(port, 3);
        let file = temp_upload_file("ok");

        let result = t.upload_file_with_retry(file.to_str().unwrap(), 0, 1);

        assert_eq!(result.unwrap(), "hello");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn test_client_error_is_structured_and_not_retried() {
        let (port, requests) =
            spawn_http_stub("HTTP/1.1 400 Bad Request", "{\"error\":\"bad model\"}");
        let t = transcriber_for_port(port, 3);
        let file = temp_upload_file("bad-request");

        let started = std::time::Instant::now();
        let result = t.upload_file_with_retry(file.to_str().unwrap(), 0, 1);

        match result {
            Err(TranscribeError::Api { status: 400, body }) => {
                assert!(body.contains("bad model"));
            }
            other => panic!("expected structured 400 error, got {:?}", other),
        }
        // No retry: a single request, no exponential-backoff sleeps.
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn test_server_error_is_retried_and_kept_structured() {
        let (port, requests) =
            spawn_http_stub("HTTP/1.1 503 Service Unavailable", "{\"error\":\"busy\"}");
        let t = transcriber_for_port(port, 1);
        let file = temp_upload_file("server-error");

        let result = t.upload_file_with_retry(file.to_str().unwrap(), 0, 1);

        assert!(matches!(
            result,
            Err(TranscribeError::Api { status: 503, .. })
        ));
        // Initial attempt + one retry.
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn test_missing_file_is_network_error() {
        let (port, _requests) = spawn_http_stub("HTTP/1.1 200 OK", "{\"text\":\"x\"}");
        let t = transcriber_for_port(port, 0);

        let result = t.upload_file_with_retry("/nonexistent/viberwhisper.wav", 0, 1);

        assert!(matches!(result, Err(TranscribeError::Network(_))));
    }
}
