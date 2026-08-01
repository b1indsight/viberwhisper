use crate::core::config::ApiAuth;
use crate::postprocess::{LlmConfig, PostProcessError};
use reqwest::blocking::Client;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
use tracing::info;

const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

const DEFAULT_PROMPT: &str = "请将下面的语音转写结果整理为适合直接发送的中文文本：\n\
    - 保留原意，不要扩写\n\
    - 添加自然标点\n\
    - 删除无意义语气词、重复和明显自我打断\n\
    - 若句子本身不完整，可做最小必要整理\n\
    - 只输出整理后的最终文本，不要解释";

pub struct LlmPostProcessor {
    auth: ApiAuth,
    api_url: reqwest::Url,
    model: String,
    prompt: String,
    temperature: f32,
    streaming_enabled: bool,
    client: Client,
}

impl LlmPostProcessor {
    pub(crate) fn new(config: LlmConfig) -> Result<Self, reqwest::Error> {
        let prompt = config.prompt.unwrap_or_else(|| DEFAULT_PROMPT.to_string());
        Ok(Self {
            auth: config.auth,
            api_url: config.endpoint,
            model: config.model,
            prompt,
            temperature: config.temperature,
            streaming_enabled: config.preheat_enabled,
            client: Client::builder().timeout(LLM_REQUEST_TIMEOUT).build()?,
        })
    }

    fn call_llm(&self, text: &str) -> Result<String, PostProcessError> {
        call_llm_impl(
            &self.client,
            &self.auth,
            &self.api_url,
            &self.model,
            &self.prompt,
            self.temperature,
            text,
        )
    }

    pub(crate) fn process(&self, text: &str) -> Result<String, PostProcessError> {
        if text.is_empty() {
            return Ok(text.to_string());
        }
        info!(text_len = text.len(), "Post-processing text with LLM");
        let result = self.call_llm(text)?;
        info!(result_len = result.len(), "LLM post-processing complete");
        Ok(result)
    }

    pub(crate) fn start_session(&self) -> LlmSession {
        let session = if self.streaming_enabled {
            LlmSessionKind::Preheat(PreheatLlmSession::new(
                self.auth.clone(),
                self.api_url.clone(),
                self.model.clone(),
                self.prompt.clone(),
                self.temperature,
                self.client.clone(),
            ))
        } else {
            LlmSessionKind::Conservative(ConservativeLlmSession {
                auth: self.auth.clone(),
                api_url: self.api_url.clone(),
                model: self.model.clone(),
                prompt: self.prompt.clone(),
                temperature: self.temperature,
                client: self.client.clone(),
                chunks: Vec::new(),
            })
        };
        LlmSession(session)
    }
}

fn call_llm_impl(
    client: &Client,
    auth: &ApiAuth,
    api_url: &reqwest::Url,
    model: &str,
    prompt: &str,
    temperature: f32,
    text: &str,
) -> Result<String, PostProcessError> {
    let request_body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": text}
        ],
        "temperature": temperature,
        "stream": false
    });

    let mut request = client
        .post(api_url.clone())
        .header("Content-Type", "application/json")
        .json(&request_body);
    if let ApiAuth::Bearer(secret) = auth {
        request = request.bearer_auth(secret.expose());
    }
    let response = request.send()?;

    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(PostProcessError::Api {
            status: status.as_u16(),
            body,
        });
    }

    let json: serde_json::Value = serde_json::from_str(&body)?;
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or(PostProcessError::MissingContent)?
        .trim()
        .to_string();

    if content.is_empty() {
        return Err(PostProcessError::EmptyContent);
    }

    Ok(content)
}

// ---------------------------------------------------------------------------
// Conservative session (streaming_enabled = false): accumulate, call once.
// ---------------------------------------------------------------------------

struct ConservativeLlmSession {
    auth: ApiAuth,
    api_url: reqwest::Url,
    model: String,
    prompt: String,
    temperature: f32,
    client: Client,
    chunks: Vec<String>,
}

impl ConservativeLlmSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> Result<String, PostProcessError> {
        // Post-processing receives already ordered stable text fragments; joining
        // without separators preserves the STT layer's chosen spacing/punctuation.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(combined);
        }
        call_llm_impl(
            &self.client,
            &self.auth,
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
// ---------------------------------------------------------------------------

/// Shared state between the session and its background LLM threads.
struct PreheatState {
    /// The generation counter for the latest request.
    latest_generation: u64,
    /// Result from the latest completed request whose generation matches `latest_generation`.
    latest_result: Option<Result<String, String>>,
}

struct PreheatLlmSession {
    auth: ApiAuth,
    api_url: reqwest::Url,
    model: String,
    prompt: String,
    temperature: f32,
    client: Client,
    chunks: Vec<String>,
    generation: u64,
    state: Arc<(Mutex<PreheatState>, Condvar)>,
}

impl PreheatLlmSession {
    fn new(
        auth: ApiAuth,
        api_url: reqwest::Url,
        model: String,
        prompt: String,
        temperature: f32,
        client: Client,
    ) -> Self {
        Self {
            auth,
            api_url,
            model,
            prompt,
            temperature,
            client,
            chunks: Vec::new(),
            generation: 0,
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

        let auth = self.auth.clone();
        let api_url = self.api_url.clone();
        let model = self.model.clone();
        let prompt = self.prompt.clone();
        let temperature = self.temperature;
        let client = self.client.clone();
        let state = Arc::clone(&self.state);

        thread::spawn(move || {
            let result = call_llm_impl(
                &client,
                &auth,
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

impl PreheatLlmSession {
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

    fn finish(&mut self) -> Result<String, PostProcessError> {
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
                &self.auth,
                &self.api_url,
                &self.model,
                &self.prompt,
                self.temperature,
                &combined,
            );
        }

        // Wait for the latest generation's result.
        let (lock, cvar) = &*self.state;
        let mut st = lock.lock().unwrap();
        while st.latest_result.is_none() {
            st = cvar.wait(st).unwrap();
        }

        let result = st.latest_result.take().unwrap();
        match result {
            Ok(text) => {
                info!(
                    result_len = text.len(),
                    "Preheat: LLM post-processing complete"
                );
                Ok(text)
            }
            Err(e) => {
                // Preheat request failed — retry once with full text as fallback.
                info!(error = %e, "Preheat: last request failed, retrying with full text");
                drop(st);
                call_llm_impl(
                    &self.client,
                    &self.auth,
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

pub(crate) struct LlmSession(LlmSessionKind);

enum LlmSessionKind {
    Conservative(ConservativeLlmSession),
    Preheat(PreheatLlmSession),
}

impl LlmSession {
    pub(crate) fn push_stable_chunk(&mut self, text: &str) {
        match &mut self.0 {
            LlmSessionKind::Conservative(session) => session.push_stable_chunk(text),
            LlmSessionKind::Preheat(session) => session.push_stable_chunk(text),
        }
    }

    pub(crate) fn finish(&mut self) -> Result<String, PostProcessError> {
        match &mut self.0 {
            LlmSessionKind::Conservative(session) => session.finish(),
            LlmSessionKind::Preheat(session) => session.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{ApiAuth, PostProcessSection, SecretValue};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn config_with_postprocess(preheat_enabled: bool, prompt: Option<&str>) -> LlmConfig {
        let section = PostProcessSection {
            enabled: true,
            preheat_enabled,
            prompt: prompt.map(str::to_string),
            temperature: 0.0,
        };
        match crate::postprocess::PostProcessConfig::validate(
            Some("https://api.example.com/v1/chat/completions"),
            ApiAuth::Bearer(SecretValue::new("test_key")),
            Some("gpt-4o-mini"),
            &section,
        )
        .unwrap()
        {
            crate::postprocess::PostProcessConfig::Llm(config) => config,
            crate::postprocess::PostProcessConfig::Disabled => unreachable!(),
        }
    }

    fn spawn_header_stub() -> (reqwest::Url, mpsc::Receiver<String>) {
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
            let headers = String::from_utf8_lossy(&request)
                .split("\r\n\r\n")
                .next()
                .unwrap_or_default()
                .to_string();
            sender.send(headers).unwrap();
            let body = r#"{"choices":[{"message":{"content":"clean"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (
            reqwest::Url::parse(&format!("http://127.0.0.1:{port}/v1/chat/completions")).unwrap(),
            receiver,
        )
    }

    #[test]
    fn authorization_header_matches_typed_auth_mode() {
        let client = Client::builder().build().unwrap();
        for (auth, expected_header) in [
            (ApiAuth::None, None),
            (
                ApiAuth::Bearer(SecretValue::new("header-token")),
                Some("authorization: bearer header-token"),
            ),
        ] {
            let (endpoint, headers) = spawn_header_stub();
            assert_eq!(
                call_llm_impl(&client, &auth, &endpoint, "model", "prompt", 0.0, "raw").unwrap(),
                "clean"
            );
            let headers = headers.recv().unwrap().to_ascii_lowercase();
            match expected_header {
                Some(expected) => assert!(headers.contains(expected)),
                None => assert!(!headers.contains("authorization:")),
            }
        }
    }

    #[test]
    fn configures_prompt_and_session_mode() {
        let p = LlmPostProcessor::new(config_with_postprocess(true, None)).unwrap();
        assert_eq!(p.model, "gpt-4o-mini");
        assert_eq!(
            p.api_url.as_str(),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(p.prompt, DEFAULT_PROMPT);
        assert!(p.streaming_enabled);

        let custom =
            LlmPostProcessor::new(config_with_postprocess(false, Some("custom prompt"))).unwrap();
        assert_eq!(custom.prompt, "custom prompt");
        assert!(!custom.streaming_enabled);
    }

    #[test]
    fn test_process_empty_text_bypasses_llm() {
        let config = config_with_postprocess(true, None);
        let p = LlmPostProcessor::new(config).unwrap();
        let result = p.process("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    // --- Conservative session tests ---

    #[test]
    fn test_conservative_session_no_chunks_finish_empty() {
        let config = config_with_postprocess(false, None);
        let p = LlmPostProcessor::new(config).unwrap();
        let mut session = p.start_session();
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    // --- Preheat session tests ---

    #[test]
    fn test_preheat_session_no_chunks_finish_empty() {
        let config = config_with_postprocess(true, None);
        let p = LlmPostProcessor::new(config).unwrap();
        let mut session = p.start_session();
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }
}
