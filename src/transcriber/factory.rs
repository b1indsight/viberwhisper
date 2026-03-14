use crate::core::config::AppConfig;
use super::groq::{GroqTranscriber, MockTranscriber, Transcriber};
use tracing::{info, warn};

/// Create a transcriber based on `config.provider`.
///
/// This is the single extension point for adding new providers: add a new
/// match arm that constructs the appropriate `Box<dyn Transcriber>`.
///
/// Falls back to `MockTranscriber` when:
/// - The provider is `"groq"` but no API key is configured
/// - The provider name is unrecognized
pub fn create_transcriber(config: &AppConfig) -> Box<dyn Transcriber> {
    match config.provider.as_str() {
        "groq" => match GroqTranscriber::from_config(config) {
            Ok(t) => {
                info!(model = %config.model, "Using Groq Whisper for speech recognition");
                Box::new(t)
            }
            Err(e) => {
                warn!(error = %e, "Failed to initialize Groq, falling back to Mock mode");
                Box::new(MockTranscriber)
            }
        },
        other => {
            warn!(provider = %other, "Unknown provider, falling back to Mock mode");
            Box::new(MockTranscriber)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    #[test]
    fn test_create_transcriber_groq_no_key_falls_back_to_mock() {
        let config = AppConfig::default(); // provider="groq", no api key
        // Should not panic; falls back to MockTranscriber
        let t = create_transcriber(&config);
        let result = t.transcribe("fake.wav");
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_transcriber_unknown_provider_falls_back_to_mock() {
        let mut config = AppConfig::default();
        config.provider = "unknown_provider".to_string();
        let t = create_transcriber(&config);
        let result = t.transcribe("fake.wav");
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_transcriber_groq_with_key() {
        let mut config = AppConfig::default();
        config.groq_api_key = Some("test_key".to_string());
        // Should not panic; returns a GroqTranscriber (won't actually call API)
        let _t = create_transcriber(&config);
    }
}
