use crate::config::AppConfig;
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

pub struct GroqTranscriber {
    api_key: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
}

impl GroqTranscriber {
    pub fn from_config(config: &AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let api_key = config
            .groq_api_key
            .clone()
            .ok_or("GROQ_API_KEY not configured (set in config.json or env var)")?;
        Ok(Self {
            api_key,
            model: config.model.clone(),
            language: config.language.clone(),
            prompt: config.prompt.clone(),
            temperature: config.temperature,
        })
    }
}

impl Transcriber for GroqTranscriber {
    #[instrument(name = "groq_stt", skip(self), fields(path = %wav_path))]
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
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()?;

        let status = response.status();
        let body = response.text()?;

        if !status.is_success() {
            return Err(format!("Groq API error {}: {}", status, body).into());
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

    #[test]
    fn test_mock_transcriber_returns_text() {
        let t = MockTranscriber;
        let result = t.transcribe("fake.wav");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }
}
