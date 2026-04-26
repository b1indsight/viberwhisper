use super::api::{ApiTranscriber, MockTranscriber, Transcriber};
use crate::core::config::AppConfig;
use tracing::{info, warn};

/// Create a transcriber from config.
///
/// Attempts to initialize `ApiTranscriber` using `config.api_key` and
/// `config.transcription_api_url`. In debug/test builds, falls back to
/// `MockTranscriber` when no API key is configured. In release builds, returns
/// the initialization error so missing transcription configuration is explicit.
pub fn create_transcriber(
    config: &AppConfig,
) -> Result<Box<dyn Transcriber>, Box<dyn std::error::Error>> {
    match ApiTranscriber::from_config(config) {
        Ok(t) => {
            info!(
                model = %config.model,
                api_url = %config.transcription_api_url,
                "Using API transcriber for speech recognition"
            );
            Ok(Box::new(t))
        }
        Err(e) => {
            if cfg!(any(debug_assertions, test)) {
                warn!(error = %e, "Failed to initialize API transcriber, falling back to Mock mode");
                Ok(Box::new(MockTranscriber))
            } else {
                Err(e)
            }
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
        let t = create_transcriber(&config).unwrap();
        // MockTranscriber still returns ok
        let result = t.transcribe("fake.wav");
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_transcriber_with_key() {
        let config = AppConfig {
            api_key: Some("test_key".to_string()),
            ..Default::default()
        };
        // Returns ApiTranscriber; won't actually call the API
        let _t = create_transcriber(&config).unwrap();
    }

    #[test]
    fn test_create_transcriber_groq_api_key_compat() {
        // Simulate GROQ_API_KEY being set via config compat path
        let config = AppConfig {
            api_key: Some("gsk_compat_key".to_string()),
            ..Default::default()
        };
        let t = create_transcriber(&config).unwrap();
        // ApiTranscriber: transcribing a nonexistent file fails with IO error, not mock text
        let result = t.transcribe("/nonexistent/path.wav");
        assert!(result.is_err());
    }
}
