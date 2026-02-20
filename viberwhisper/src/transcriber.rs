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
}

impl GroqTranscriber {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let api_key = std::env::var("GROQ_API_KEY")
            .map_err(|_| "环境变量 GROQ_API_KEY 未设置")?;
        Ok(Self::new(api_key))
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

        let form = reqwest::blocking::multipart::Form::new()
            .text("model", "whisper-large-v3-turbo")
            .text("temperature", "0")
            .text("response_format", "verbose_json")
            .part("file", part);

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
