use crate::core::config::AppConfig;
use crate::postprocess::{TextPostProcessor, TextPostProcessorSession};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use tracing::info;

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
        })
    }

    fn call_llm(&self, text: &str) -> Result<String, Box<dyn std::error::Error>> {
        call_llm_impl(
            &self.api_key,
            &self.api_url,
            &self.model,
            &self.prompt,
            self.temperature,
            text,
        )
    }
}

fn call_llm_impl(
    api_key: &str,
    api_url: &str,
    model: &str,
    prompt: &str,
    temperature: f32,
    text: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let request_body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": text}
        ],
        "temperature": temperature,
        "stream": false
    });

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()?;

    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(format!("LLM API error {}: {}", status, body).into());
    }

    let json: serde_json::Value = serde_json::from_str(&body)?;
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("content field not found in LLM response")?
        .trim()
        .to_string();

    if content.is_empty() {
        return Err("LLM returned empty content".into());
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
            ))
        } else {
            Box::new(ConservativeLlmSession {
                api_key: self.api_key.clone(),
                api_url: self.api_url.clone(),
                model: self.model.clone(),
                prompt: self.prompt.clone(),
                temperature: self.temperature,
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
    chunks: Vec<String>,
}

impl TextPostProcessorSession for ConservativeLlmSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(combined);
        }
        call_llm_impl(
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
    chunks: Vec<String>,
    generation: u64,
    state: Arc<(Mutex<PreheatState>, Condvar)>,
}

impl PreheatLlmSession {
    fn new(
        api_key: String,
        api_url: String,
        model: String,
        prompt: String,
        temperature: f32,
    ) -> Self {
        Self {
            api_key,
            api_url,
            model,
            prompt,
            temperature,
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
        let state = Arc::clone(&self.state);

        thread::spawn(move || {
            let result = call_llm_impl(&api_key, &api_url, &model, &prompt, temperature, &combined);

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
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(combined);
        }

        // If no request was ever fired (shouldn't happen if push_stable_chunk was called
        // with non-empty text, but be safe), fire one now.
        if self.generation == 0 {
            return call_llm_impl(
                &self.api_key,
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

    #[test]
    fn test_preheat_state_generation_increments() {
        let mut session = PreheatLlmSession::new(
            "key".to_string(),
            "http://localhost:1/v1/chat/completions".to_string(),
            "model".to_string(),
            "prompt".to_string(),
            0.0,
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
