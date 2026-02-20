use crate::config::AppConfig;

pub trait Transcriber {
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>>;
}

pub struct MockTranscriber;

impl Transcriber for MockTranscriber {
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
        println!("[Mock STT] 正在识别: {}", wav_path);
        let text = "这是一段模拟识别出来的文字".to_string();
        println!("[Mock STT] 识别结果: {}", text);
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
            .ok_or("GROQ_API_KEY 未配置（config.json 或环境变量）")?;
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
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>> {
        println!("[Groq STT] 正在识别: {}", wav_path);

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
            return Err(format!("Groq API 错误 {}: {}", status, body).into());
        }

        let json: serde_json::Value = serde_json::from_str(&body)?;
        let text = json["text"]
            .as_str()
            .ok_or("响应中未找到 text 字段")?
            .trim()
            .to_string();

        println!("[Groq STT] 识别结果: {}", text);
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
