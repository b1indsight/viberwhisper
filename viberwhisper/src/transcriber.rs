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
