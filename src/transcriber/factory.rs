use super::api::{ApiTranscriber, MockTranscriber, Transcriber};
use crate::core::config::AppConfig;
use tracing::{info, warn};

/// Create a transcriber from config.
///
/// Attempts to initialize `ApiTranscriber` using `config.api_key` and
/// `config.transcription_api_url`. Falls back to `MockTranscriber` when no
/// API key is configured.
pub fn create_transcriber(config: &AppConfig) -> Box<dyn Transcriber> {
    match ApiTranscriber::from_config(config) {
        Ok(t) => {
            info!(
                model = %config.model,
                api_url = %config.transcription_api_url,
                "Using API transcriber for speech recognition"
            );
            Box::new(t)
        }
        Err(e) => {
            warn!(error = %e, "Failed to initialize API transcriber, falling back to Mock mode");
            Box::new(MockTranscriber)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    #[test]
    fn test_create_transcriber_no_key_falls_back_to_mock() {
        let config = AppConfig::default(); // no api_key
        let t = create_transcriber(&config);
        // MockTranscriber still returns ok
        let result = t.transcribe("fake.wav");
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_transcriber_with_key() {
        let mut config = AppConfig::default();
        config.api_key = Some("test_key".to_string());
        // Returns ApiTranscriber; won't actually call the API
        let _t = create_transcriber(&config);
    }

    #[test]
    fn test_create_transcriber_groq_api_key_compat() {
        // Simulate GROQ_API_KEY being set via config compat path
        let mut config = AppConfig::default();
        config.api_key = Some("gsk_compat_key".to_string());
        let t = create_transcriber(&config);
        // ApiTranscriber: transcribing a nonexistent file fails with IO error, not mock text
        let result = t.transcribe("/nonexistent/path.wav");
        assert!(result.is_err());
    }
}
