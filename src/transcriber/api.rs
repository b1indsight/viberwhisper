use crate::audio::split_wav;
use crate::core::config::AppConfig;
use crate::local::LocalServiceManager;
use std::path::PathBuf;
use tracing::{info, instrument, warn};

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>>;
}

pub struct MockTranscriber;

impl Transcriber for MockTranscriber {
    #[instrument(name = "mock_stt", skip(self), fields(path = %wav_path))]
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
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
    local_service: Option<LocalServiceDiagnostics>,
}

#[derive(Clone)]
struct LocalServiceDiagnostics {
    port: u16,
    log_file: PathBuf,
    pid_file: PathBuf,
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
            local_service: local_service_diagnostics(config),
        })
    }

    /// Upload a single WAV file and return its transcription text.
    fn upload_file(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
        let file_bytes = std::fs::read(wav_path)?;
        let file_name = std::path::Path::new(wav_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.wav")
            .to_string();

        let part = reqwest::blocking::multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str("audio/wav")?;

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

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()?;

        let status = response.status();
        let body = response.text()?;

        if !status.is_success() {
            return Err(format!("API error {}: {}", status, body).into());
        }

        let json: serde_json::Value = serde_json::from_str(&body)?;
        let text = json["text"]
            .as_str()
            .ok_or("text field not found in response")?
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
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut last_error: Box<dyn std::error::Error> = "upload not attempted".to_string().into();

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

            // We need to distinguish 4xx from 5xx / network errors.
            // Read file and do the request manually so we can inspect the status.
            match self.try_upload(wav_path) {
                Ok(text) => return Ok(text),
                Err(e) => {
                    let enriched_error = self.enrich_error(e);
                    // Extract HTTP status from error message if present.
                    let msg = enriched_error.to_string();
                    // Parse "API error 4XX: ..." — do not retry 4xx.
                    if let Some(status_str) = msg.strip_prefix("API error ")
                        && let Some(code_str) = status_str.split(':').next()
                        && let Ok(code) = code_str.trim().parse::<u16>()
                        && !Self::is_retryable_status(code)
                    {
                        return Err(enriched_error);
                    }
                    warn!(
                        chunk = chunk_index + 1,
                        total = total_chunks,
                        attempt = attempt,
                        error = %enriched_error,
                        "Chunk upload failed"
                    );
                    last_error = enriched_error;
                }
            }
        }

        Err(format!(
            "chunk {}/{} failed after {} attempts: {}",
            chunk_index + 1,
            total_chunks,
            self.max_retries + 1,
            last_error
        )
        .into())
    }

    /// Low-level upload attempt (one shot, no retry).
    fn try_upload(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
        self.upload_file(wav_path)
    }

    fn enrich_error(&self, error: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
        let Some(local_service) = &self.local_service else {
            return error;
        };

        let base_message = error.to_string();
        let diagnostics = local_service.describe_failure();
        format!("{base_message}; {diagnostics}").into()
    }
}

/// Merge transcription results from multiple chunks.
///
/// Chinese text (zh, zh-CN, zh-TW) is concatenated without a separator.
/// All other languages use a single space as separator.
fn merge_texts(texts: &[String], language: Option<&str>) -> String {
    let separator = match language {
        Some(lang) if lang.starts_with("zh") => "",
        _ => " ",
    };
    texts
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(separator)
}

impl LocalServiceDiagnostics {
    fn describe_failure(&self) -> String {
        let running = self
            .read_pid()
            .is_some_and(LocalServiceDiagnostics::is_pid_running);
        let health = self.health();
        format!(
            "local service diagnostics: running={running}, port={}, health={}, log_file={}",
            self.port,
            health,
            self.log_file.display()
        )
    }

    fn health(&self) -> String {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
        {
            Ok(client) => client,
            Err(error) => return error.to_string(),
        };

        match client
            .get(format!("http://127.0.0.1:{}/health", self.port))
            .send()
        {
            Ok(resp) => format!("http {}", resp.status().as_u16()),
            Err(error) => error.to_string(),
        }
    }

    fn read_pid(&self) -> Option<u32> {
        std::fs::read_to_string(&self.pid_file)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
    }

    fn is_pid_running(pid: u32) -> bool {
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .ok()
                .map(|output| {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    stdout.contains(&format!(",\"{pid}\""))
                })
                .unwrap_or(false)
        }

        #[cfg(not(target_os = "windows"))]
        {
            std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "pid="])
                .output()
                .ok()
                .map(|output| {
                    output.status.success()
                        && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
                })
                .unwrap_or(false)
        }
    }
}

fn local_service_diagnostics(config: &AppConfig) -> Option<LocalServiceDiagnostics> {
    let localhost = format!("http://127.0.0.1:{}", config.local_server_port);
    let using_local_endpoint =
        config.local_mode || config.transcription_api_url.starts_with(&localhost);
    if !using_local_endpoint {
        return None;
    }

    let data_dir = resolve_local_data_dir(config.local_data_dir.as_deref())?;
    Some(LocalServiceDiagnostics {
        port: config.local_server_port,
        log_file: LocalServiceManager::default_log_file_path(&data_dir),
        pid_file: data_dir.join("local_server.pid"),
    })
}

fn resolve_local_data_dir(configured: Option<&str>) -> Option<PathBuf> {
    match configured {
        Some(path) if path.starts_with("~/") => dirs::home_dir().map(|home| home.join(&path[2..])),
        Some(path) => Some(PathBuf::from(path)),
        None => dirs::home_dir().map(|home| home.join(".viberwhisper")),
    }
}

impl Transcriber for ApiTranscriber {
    #[instrument(name = "api_stt", skip(self), fields(path = %wav_path))]
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
        info!("Starting transcription");

        let chunks = split_wav(
            wav_path,
            self.max_chunk_duration_secs,
            self.max_chunk_size_bytes,
        )?;

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
    use std::path::{Path, PathBuf};

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
    fn test_merge_texts_zh() {
        let texts = vec!["你好".to_string(), "世界".to_string()];
        let merged = merge_texts(&texts, Some("zh"));
        assert_eq!(merged, "你好世界");
    }

    #[test]
    fn test_merge_texts_zh_cn() {
        let texts = vec!["你好".to_string(), "世界".to_string()];
        let merged = merge_texts(&texts, Some("zh-CN"));
        assert_eq!(merged, "你好世界");
    }

    #[test]
    fn test_merge_texts_en() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        let merged = merge_texts(&texts, Some("en"));
        assert_eq!(merged, "hello world");
    }

    #[test]
    fn test_merge_texts_no_language() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        let merged = merge_texts(&texts, None);
        assert_eq!(merged, "hello world");
    }

    #[test]
    fn test_merge_texts_empty_segments_filtered() {
        let texts = vec!["hello".to_string(), "".to_string(), "world".to_string()];
        let merged = merge_texts(&texts, Some("en"));
        assert_eq!(merged, "hello world");
    }

    #[test]
    fn test_merge_texts_all_empty() {
        let texts = vec!["".to_string(), "".to_string()];
        let merged = merge_texts(&texts, Some("en"));
        assert_eq!(merged, "");
    }

    #[test]
    fn test_is_retryable_status() {
        assert!(ApiTranscriber::is_retryable_status(500));
        assert!(ApiTranscriber::is_retryable_status(503));
        assert!(!ApiTranscriber::is_retryable_status(400));
        assert!(!ApiTranscriber::is_retryable_status(404));
        assert!(!ApiTranscriber::is_retryable_status(429));
    }

    #[test]
    fn test_local_service_diagnostics_enabled_for_local_mode() {
        let config = AppConfig {
            local_mode: true,
            ..Default::default()
        };
        let diagnostics = local_service_diagnostics(&config).unwrap();
        assert_eq!(diagnostics.port, 17265);
        assert!(
            diagnostics
                .log_file
                .ends_with(Path::new(".viberwhisper/server.log"))
        );
        assert!(
            diagnostics
                .pid_file
                .ends_with(Path::new(".viberwhisper/local_server.pid"))
        );
    }

    #[test]
    fn test_enrich_error_includes_local_service_details() {
        let transcriber = ApiTranscriber {
            api_key: "key".to_string(),
            api_url: "http://127.0.0.1:17265/v1/audio/transcriptions".to_string(),
            model: "m".to_string(),
            language: None,
            prompt: None,
            temperature: 0.0,
            max_chunk_duration_secs: 30,
            max_chunk_size_bytes: 1024,
            max_retries: 1,
            local_service: Some(LocalServiceDiagnostics {
                port: 17265,
                log_file: PathBuf::from("/tmp/server.log"),
                pid_file: PathBuf::from("/tmp/local_server.pid"),
            }),
        };

        let error = transcriber.enrich_error("boom".to_string().into());
        let message = error.to_string();
        assert!(message.contains("boom"));
        assert!(message.contains("local service diagnostics:"));
        assert!(message.contains("/tmp/server.log"));
    }
}
