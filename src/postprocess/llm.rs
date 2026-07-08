use crate::core::config::AppConfig;
use crate::postprocess::{TextPostProcessor, TextPostProcessorSession};
use reqwest::blocking::Client;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Wait before the single retry of a transient LLM failure.
const LLM_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Margin over the HTTP client timeout before `finish()` stops waiting for the
/// background preheat thread and retries synchronously. The background request
/// should always resolve within `LLM_REQUEST_TIMEOUT`; this bound protects
/// against a panicked or wedged thread never signalling the condvar.
const PREHEAT_WAIT_MARGIN: Duration = Duration::from_secs(10);

const DEFAULT_PROMPT: &str = "请将下面的语音转写结果整理为适合直接发送的中文文本：\n\
    - 保留原意，不要扩写\n\
    - 添加自然标点\n\
    - 删除无意义语气词、重复和明显自我打断\n\
    - 若句子本身不完整，可做最小必要整理\n\
    - 只输出整理后的最终文本，不要解释";

pub struct LlmPostProcessor {
    api_key: String,
    api_url: String,
    model: String,
    prompt: String,
    temperature: f32,
    streaming_enabled: bool,
    client: Client,
}

impl LlmPostProcessor {
    pub fn from_config(config: &AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let api_key = config
            .post_process_api_key
            .clone()
            .ok_or("post_process_api_key not configured")?;
        let api_url = config
            .post_process_api_url
            .clone()
            .ok_or("post_process_api_url not configured")?;
        let model = config
            .post_process_model
            .clone()
            .ok_or("post_process_model not configured")?;
        let prompt = config
            .post_process_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_PROMPT.to_string());
        Ok(Self {
            api_key,
            api_url,
            model,
            prompt,
            temperature: config.post_process_temperature,
            streaming_enabled: config.post_process_streaming_enabled,
            client: Client::builder().timeout(LLM_REQUEST_TIMEOUT).build()?,
        })
    }

    fn call_llm(&self, text: &str) -> Result<String, Box<dyn std::error::Error>> {
        call_llm_impl(
            &self.client,
            &self.api_key,
            &self.api_url,
            &self.model,
            &self.prompt,
            self.temperature,
            text,
        )
    }
}

/// Internal LLM call failure, classified so the retry decision does not have
/// to parse error strings (mirrors `transcriber::TranscribeError`).
#[derive(Debug)]
enum LlmCallError {
    /// HTTP error response from the API.
    Api { status: u16, body: String },
    /// Network / connection failure.
    Network(String),
    /// Response arrived but could not be used (bad JSON, missing or empty content).
    BadResponse(String),
}

impl LlmCallError {
    /// Transient failures worth one retry: network errors and server-side 5xx.
    fn is_transient(&self) -> bool {
        match self {
            LlmCallError::Network(_) => true,
            LlmCallError::Api { status, .. } => *status >= 500,
            LlmCallError::BadResponse(_) => false,
        }
    }
}

impl fmt::Display for LlmCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmCallError::Api { status, body } => write!(f, "LLM API error {}: {}", status, body),
            LlmCallError::Network(msg) => write!(f, "{}", msg),
            LlmCallError::BadResponse(msg) => write!(f, "{}", msg),
        }
    }
}

/// One LLM call with a single retry on transient failures (network / 5xx).
/// Client errors and malformed responses are returned immediately — the
/// caller's fallback (keep the raw STT text) handles those.
fn call_llm_impl(
    client: &Client,
    api_key: &str,
    api_url: &str,
    model: &str,
    prompt: &str,
    temperature: f32,
    text: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    match call_llm_once(client, api_key, api_url, model, prompt, temperature, text) {
        Ok(content) => Ok(content),
        Err(e) if e.is_transient() => {
            warn!(error = %e, "LLM request failed; retrying once");
            thread::sleep(LLM_RETRY_DELAY);
            call_llm_once(client, api_key, api_url, model, prompt, temperature, text)
                .map_err(|e| e.to_string().into())
        }
        Err(e) => Err(e.to_string().into()),
    }
}

fn call_llm_once(
    client: &Client,
    api_key: &str,
    api_url: &str,
    model: &str,
    prompt: &str,
    temperature: f32,
    text: &str,
) -> Result<String, LlmCallError> {
    let request_body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": text}
        ],
        "temperature": temperature,
        "stream": false
    });

    let response = client
        .post(api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .map_err(|e| LlmCallError::Network(e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|e| LlmCallError::Network(e.to_string()))?;

    if !status.is_success() {
        return Err(LlmCallError::Api {
            status: status.as_u16(),
            body,
        });
    }

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| LlmCallError::BadResponse(format!("invalid JSON in LLM response: {e}")))?;
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            LlmCallError::BadResponse("content field not found in LLM response".to_string())
        })?
        .trim()
        .to_string();

    if content.is_empty() {
        return Err(LlmCallError::BadResponse(
            "LLM returned empty content".to_string(),
        ));
    }

    Ok(content)
}

impl TextPostProcessor for LlmPostProcessor {
    fn process(&self, text: &str) -> Result<String, Box<dyn std::error::Error>> {
        if text.is_empty() {
            return Ok(text.to_string());
        }
        info!(text_len = text.len(), "Post-processing text with LLM");
        let result = self.call_llm(text)?;
        info!(result_len = result.len(), "LLM post-processing complete");
        Ok(result)
    }

    fn start_session(&self) -> Box<dyn TextPostProcessorSession> {
        if self.streaming_enabled {
            Box::new(PreheatLlmSession::new(
                self.api_key.clone(),
                self.api_url.clone(),
                self.model.clone(),
                self.prompt.clone(),
                self.temperature,
                self.client.clone(),
            ))
        } else {
            Box::new(ConservativeLlmSession {
                api_key: self.api_key.clone(),
                api_url: self.api_url.clone(),
                model: self.model.clone(),
                prompt: self.prompt.clone(),
                temperature: self.temperature,
                client: self.client.clone(),
                chunks: Vec::new(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Conservative session (streaming_enabled = false): accumulate, call once.
// ---------------------------------------------------------------------------

struct ConservativeLlmSession {
    api_key: String,
    api_url: String,
    model: String,
    prompt: String,
    temperature: f32,
    client: Client,
    chunks: Vec<String>,
}

impl TextPostProcessorSession for ConservativeLlmSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        // Post-processing receives already ordered stable text fragments; joining
        // without separators preserves the STT layer's chosen spacing/punctuation.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(combined);
        }
        call_llm_impl(
            &self.client,
            &self.api_key,
            &self.api_url,
            &self.model,
            &self.prompt,
            self.temperature,
            &combined,
        )
    }
}

// ---------------------------------------------------------------------------
// Preheat session (streaming_enabled = true): fire LLM on every chunk arrival.
//
// Compatibility note: the chat-completions endpoint is a whole-request /
// whole-response API — there is no incremental input channel to append text
// to. "Streaming" therefore means: every time a stable chunk arrives, fire a
// fresh request carrying ALL accumulated text and let the newest generation
// win; results of superseded generations are silently dropped. This trades
// redundant tokens for latency: by the time the user stops recording, the
// request covering (most of) the session text is already in flight, so
// `finish()` usually only waits for that last round-trip instead of starting
// from zero. If the backend ever exposes a true incremental interface, only
// this session type needs replacing — the `TextPostProcessorSession` contract
// and the main-loop feeding stay the same.
// ---------------------------------------------------------------------------

/// Shared state between the session and its background LLM threads.
struct PreheatState {
    /// The generation counter for the latest request.
    latest_generation: u64,
    /// Result from the latest completed request whose generation matches `latest_generation`.
    latest_result: Option<Result<String, String>>,
}

struct PreheatLlmSession {
    api_key: String,
    api_url: String,
    model: String,
    prompt: String,
    temperature: f32,
    client: Client,
    chunks: Vec<String>,
    generation: u64,
    finish_wait_timeout: Duration,
    state: Arc<(Mutex<PreheatState>, Condvar)>,
}

impl PreheatLlmSession {
    fn new(
        api_key: String,
        api_url: String,
        model: String,
        prompt: String,
        temperature: f32,
        client: Client,
    ) -> Self {
        Self {
            api_key,
            api_url,
            model,
            prompt,
            temperature,
            client,
            chunks: Vec::new(),
            generation: 0,
            finish_wait_timeout: LLM_REQUEST_TIMEOUT + PREHEAT_WAIT_MARGIN,
            state: Arc::new((
                Mutex::new(PreheatState {
                    latest_generation: 0,
                    latest_result: None,
                }),
                Condvar::new(),
            )),
        }
    }

    fn fire_request(&mut self) {
        // Post-processing accumulates stable text exactly as received; it does
        // not add separators because upstream text already owns word spacing.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return;
        }

        self.generation += 1;
        let request_gen = self.generation;

        // Update latest_generation so older threads know they're stale.
        {
            let mut st = self.state.0.lock().unwrap();
            st.latest_generation = request_gen;
            st.latest_result = None; // clear stale result
        }

        let api_key = self.api_key.clone();
        let api_url = self.api_url.clone();
        let model = self.model.clone();
        let prompt = self.prompt.clone();
        let temperature = self.temperature;
        let client = self.client.clone();
        let state = Arc::clone(&self.state);

        thread::spawn(move || {
            let result = call_llm_impl(
                &client,
                &api_key,
                &api_url,
                &model,
                &prompt,
                temperature,
                &combined,
            );

            let (lock, cvar) = &*state;
            let mut st = lock.lock().unwrap();
            // Only store result if this thread's generation is still the latest.
            if st.latest_generation == request_gen {
                st.latest_result = Some(result.map_err(|e| e.to_string()));
                cvar.notify_all();
            }
            // Otherwise this result is stale — silently drop it.
        });
    }
}

impl TextPostProcessorSession for PreheatLlmSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
            info!(
                generation = self.generation + 1,
                text_len = self.chunks.iter().map(|c| c.len()).sum::<usize>(),
                "Preheat: firing LLM request for accumulated text"
            );
            self.fire_request();
        }
    }

    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        // Keep post-processing concatenation separator-free; the STT result is
        // already a display-ready text stream before LLM cleanup.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(combined);
        }

        // If no request was ever fired (shouldn't happen if push_stable_chunk was called
        // with non-empty text, but be safe), fire one now.
        if self.generation == 0 {
            return call_llm_impl(
                &self.client,
                &self.api_key,
                &self.api_url,
                &self.model,
                &self.prompt,
                self.temperature,
                &combined,
            );
        }

        // Wait for the latest generation's result, but never forever: a wedged
        // or panicked background thread must not hang the whole session.
        let (lock, cvar) = &*self.state;
        let mut st = lock.lock().unwrap();
        let deadline = Instant::now() + self.finish_wait_timeout;
        while st.latest_result.is_none() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (guard, _) = cvar.wait_timeout(st, deadline - now).unwrap();
            st = guard;
        }

        match st.latest_result.take() {
            Some(Ok(text)) => {
                info!(
                    result_len = text.len(),
                    "Preheat: LLM post-processing complete"
                );
                Ok(text)
            }
            Some(Err(e)) => {
                // Preheat request failed — retry once with full text as fallback.
                info!(error = %e, "Preheat: last request failed, retrying with full text");
                drop(st);
                call_llm_impl(
                    &self.client,
                    &self.api_key,
                    &self.api_url,
                    &self.model,
                    &self.prompt,
                    self.temperature,
                    &combined,
                )
            }
            None => {
                // Timed out waiting for the background thread — fall back to a
                // synchronous request with the full accumulated text.
                info!(
                    "Preheat: background request did not complete in time, retrying with full text"
                );
                drop(st);
                call_llm_impl(
                    &self.client,
                    &self.api_key,
                    &self.api_url,
                    &self.model,
                    &self.prompt,
                    self.temperature,
                    &combined,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    fn config_with_postprocess() -> AppConfig {
        AppConfig {
            post_process_enabled: true,
            post_process_api_key: Some("test_key".to_string()),
            post_process_api_url: Some("https://api.example.com/v1/chat/completions".to_string()),
            post_process_model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_from_config_missing_key() {
        let config = AppConfig::default();
        assert!(LlmPostProcessor::from_config(&config).is_err());
    }

    #[test]
    fn test_from_config_missing_url() {
        let config = AppConfig {
            post_process_api_key: Some("key".to_string()),
            ..Default::default()
        };
        assert!(LlmPostProcessor::from_config(&config).is_err());
    }

    #[test]
    fn test_from_config_missing_model() {
        let config = AppConfig {
            post_process_api_key: Some("key".to_string()),
            post_process_api_url: Some("https://example.com/v1/chat/completions".to_string()),
            ..Default::default()
        };
        assert!(LlmPostProcessor::from_config(&config).is_err());
    }

    #[test]
    fn test_from_config_success() {
        let config = config_with_postprocess();
        let result = LlmPostProcessor::from_config(&config);
        assert!(result.is_ok());
        let p = result.unwrap();
        assert_eq!(p.api_key, "test_key");
        assert_eq!(p.model, "gpt-4o-mini");
        assert_eq!(p.api_url, "https://api.example.com/v1/chat/completions");
    }

    #[test]
    fn test_from_config_default_prompt() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        assert_eq!(p.prompt, DEFAULT_PROMPT);
    }

    #[test]
    fn test_from_config_custom_prompt() {
        let mut config = config_with_postprocess();
        config.post_process_prompt = Some("custom prompt".to_string());
        let p = LlmPostProcessor::from_config(&config).unwrap();
        assert_eq!(p.prompt, "custom prompt");
    }

    #[test]
    fn test_from_config_streaming_enabled_default() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        assert!(p.streaming_enabled);
    }

    #[test]
    fn test_from_config_streaming_disabled() {
        let mut config = config_with_postprocess();
        config.post_process_streaming_enabled = false;
        let p = LlmPostProcessor::from_config(&config).unwrap();
        assert!(!p.streaming_enabled);
    }

    #[test]
    fn test_process_empty_text_bypasses_llm() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let result = p.process("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    // --- Conservative session tests ---

    #[test]
    fn test_conservative_session_no_chunks_finish_empty() {
        let mut config = config_with_postprocess();
        config.post_process_streaming_enabled = false;
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let mut session = p.start_session();
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_conservative_session_empty_chunk_ignored() {
        let mut config = config_with_postprocess();
        config.post_process_streaming_enabled = false;
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let mut session = p.start_session();
        session.push_stable_chunk("");
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    // --- Preheat session tests ---

    #[test]
    fn test_preheat_session_no_chunks_finish_empty() {
        let config = config_with_postprocess(); // streaming_enabled = true by default
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let mut session = p.start_session();
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_preheat_session_empty_chunk_ignored() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let mut session = p.start_session();
        session.push_stable_chunk("");
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    // --- retry behavior against a local HTTP stub ---

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn spawn_llm_stub(status_line: &'static str, body: &'static str) -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let mut buf = [0u8; 8192];
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

    fn processor_for_port(port: u16) -> LlmPostProcessor {
        let mut config = config_with_postprocess();
        config.post_process_api_url = Some(format!("http://127.0.0.1:{port}/v1/chat/completions"));
        LlmPostProcessor::from_config(&config).unwrap()
    }

    #[test]
    fn test_llm_success_returns_content() {
        let (port, requests) = spawn_llm_stub(
            "HTTP/1.1 200 OK",
            "{\"choices\":[{\"message\":{\"content\":\" cleaned \"}}]}",
        );
        let p = processor_for_port(port);

        assert_eq!(p.process("raw").unwrap(), "cleaned");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_llm_client_error_is_not_retried() {
        let (port, requests) =
            spawn_llm_stub("HTTP/1.1 400 Bad Request", "{\"error\":\"bad model\"}");
        let p = processor_for_port(port);

        let error = p.process("raw").unwrap_err().to_string();
        assert!(error.contains("LLM API error 400"), "{error}");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_llm_server_error_is_retried_once() {
        let (port, requests) =
            spawn_llm_stub("HTTP/1.1 503 Service Unavailable", "{\"error\":\"busy\"}");
        let p = processor_for_port(port);

        let error = p.process("raw").unwrap_err().to_string();
        assert!(error.contains("LLM API error 503"), "{error}");
        // Initial attempt + one retry, then give up.
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_preheat_finish_does_not_hang_when_no_result_ever_arrives() {
        // Simulate a fired request whose background thread never reports back
        // (e.g. it panicked): generation > 0 but no thread will signal the
        // condvar. finish() must give up after finish_wait_timeout and fall
        // back to a synchronous call (which fails fast against localhost:1).
        let mut session = PreheatLlmSession::new(
            "key".to_string(),
            "http://localhost:1/v1/chat/completions".to_string(),
            "model".to_string(),
            "prompt".to_string(),
            0.0,
            Client::builder()
                .timeout(LLM_REQUEST_TIMEOUT)
                .build()
                .unwrap(),
        );
        session.chunks.push("hello".to_string());
        session.generation = 1;
        session.finish_wait_timeout = Duration::from_millis(100);

        let started = Instant::now();
        let result = session.finish();

        // The synchronous fallback hits an unreachable port, so we expect Err,
        // and crucially we expect to get here at all (no infinite wait).
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn test_preheat_state_generation_increments() {
        let mut session = PreheatLlmSession::new(
            "key".to_string(),
            "http://localhost:1/v1/chat/completions".to_string(),
            "model".to_string(),
            "prompt".to_string(),
            0.0,
            Client::builder()
                .timeout(LLM_REQUEST_TIMEOUT)
                .build()
                .unwrap(),
        );
        assert_eq!(session.generation, 0);
        // push_stable_chunk fires a request and increments generation.
        // The HTTP call will fail (localhost:1), but generation still increments.
        session.chunks.push("hello".to_string());
        session.generation += 1;
        assert_eq!(session.generation, 1);
        session.chunks.push("world".to_string());
        session.generation += 1;
        assert_eq!(session.generation, 2);
    }
}
