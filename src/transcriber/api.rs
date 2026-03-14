use crate::core::config::AppConfig;
use tracing::{info, instrument};

pub trait Transcriber {
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
pub struct ApiTranscriber {
    api_key: String,
    api_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
}

impl ApiTranscriber {
    pub fn from_config(config: &AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let api_key = config.api_key.clone().ok_or(
            "api_key not configured (set api_key in config.json or GROQ_API_KEY env var)",
        )?;
        Ok(Self {
            api_key,
            api_url: config.transcription_api_url.clone(),
            model: config.model.clone(),
            language: config.language.clone(),
            prompt: config.prompt.clone(),
            temperature: config.temperature,
        })
    }
}

impl Transcriber for ApiTranscriber {
    #[instrument(name = "api_stt", skip(self), fields(path = %wav_path))]
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
        info!("Starting transcription");

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

        info!(result = %text, "Transcription complete");
        Ok(text)
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
        let mut config = AppConfig::default();
        config.api_key = Some("test_key".to_string());
        let result = ApiTranscriber::from_config(&config);
        assert!(result.is_ok());
        let t = result.unwrap();
        assert_eq!(t.api_key, "test_key");
        assert_eq!(
            t.api_url,
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(t.model, "whisper-large-v3-turbo");
    }

    #[test]
    fn test_api_transcriber_custom_url() {
        let mut config = AppConfig::default();
        config.api_key = Some("key".to_string());
        config.transcription_api_url =
            "https://api.openai.com/v1/audio/transcriptions".to_string();
        let t = ApiTranscriber::from_config(&config).unwrap();
        assert_eq!(t.api_url, "https://api.openai.com/v1/audio/transcriptions");
    }
}
