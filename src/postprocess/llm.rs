use crate::core::config::AppConfig;
use crate::postprocess::{TextPostProcessor, TextPostProcessorSession};
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
        })
    }

    fn call_llm(&self, text: &str) -> Result<String, Box<dyn std::error::Error>> {
        let request_body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": self.prompt},
                {"role": "user", "content": text}
            ],
            "temperature": self.temperature,
            "stream": false
        });

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
        Box::new(LlmSession {
            api_key: self.api_key.clone(),
            api_url: self.api_url.clone(),
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            temperature: self.temperature,
            chunks: Vec::new(),
        })
    }
}

/// Accumulates stable STT chunks; calls LLM once on the combined text in `finish`.
struct LlmSession {
    api_key: String,
    api_url: String,
    model: String,
    prompt: String,
    temperature: f32,
    chunks: Vec<String>,
}

impl TextPostProcessorSession for LlmSession {
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
        let processor = LlmPostProcessor {
            api_key: self.api_key.clone(),
            api_url: self.api_url.clone(),
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            temperature: self.temperature,
        };
        processor.call_llm(&combined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    fn config_with_postprocess() -> AppConfig {
        let mut config = AppConfig::default();
        config.post_process_enabled = true;
        config.post_process_api_key = Some("test_key".to_string());
        config.post_process_api_url =
            Some("https://api.example.com/v1/chat/completions".to_string());
        config.post_process_model = Some("gpt-4o-mini".to_string());
        config
    }

    #[test]
    fn test_from_config_missing_key() {
        let config = AppConfig::default();
        assert!(LlmPostProcessor::from_config(&config).is_err());
    }

    #[test]
    fn test_from_config_missing_url() {
        let mut config = AppConfig::default();
        config.post_process_api_key = Some("key".to_string());
        assert!(LlmPostProcessor::from_config(&config).is_err());
    }

    #[test]
    fn test_from_config_missing_model() {
        let mut config = AppConfig::default();
        config.post_process_api_key = Some("key".to_string());
        config.post_process_api_url =
            Some("https://example.com/v1/chat/completions".to_string());
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
        assert_eq!(
            p.api_url,
            "https://api.example.com/v1/chat/completions"
        );
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
    fn test_process_empty_text_bypasses_llm() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        // Empty text bypasses the LLM call (no network access needed).
        let result = p.process("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_session_no_chunks_finish_empty() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let mut session = p.start_session();
        // No chunks — finish returns empty without calling LLM.
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_session_empty_chunk_ignored() {
        let config = config_with_postprocess();
        let p = LlmPostProcessor::from_config(&config).unwrap();
        let mut session = p.start_session();
        session.push_stable_chunk("");
        // Still no non-empty chunks — finish returns empty without calling LLM.
        let result = session.finish();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }
}
