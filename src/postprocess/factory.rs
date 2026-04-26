use crate::core::config::AppConfig;
use crate::postprocess::{LlmPostProcessor, NoopPostProcessor, TextPostProcessor};
use tracing::warn;

/// Create a post-processor from config.
///
/// Returns `NoopPostProcessor` if post-processing is disabled or misconfigured,
/// ensuring the main pipeline is never blocked by a missing LLM setup.
///
/// Error layering:
/// - configuration errors fall back to noop here because post-processing is optional;
/// - runtime LLM request errors are returned by the processor and handled by callers;
/// - empty LLM outputs are treated as runtime errors by `LlmPostProcessor`.
pub fn create_post_processor(config: &AppConfig) -> Box<dyn TextPostProcessor> {
    if !config.post_process_enabled {
        return Box::new(NoopPostProcessor);
    }

    match LlmPostProcessor::from_config(config) {
        Ok(processor) => Box::new(processor),
        Err(e) => {
            // Keep this fallback quiet for users who enable local transcription
            // without enabling an LLM cleanup service; STT remains usable.
            warn!(error = %e, "Failed to create LLM post-processor, falling back to noop");
            Box::new(NoopPostProcessor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    #[test]
    fn test_disabled_returns_noop() {
        let config = AppConfig::default(); // post_process_enabled = false
        let p = create_post_processor(&config);
        assert_eq!(p.process("hello").unwrap(), "hello");
    }

    #[test]
    fn test_enabled_incomplete_config_falls_back_to_noop() {
        let config = AppConfig {
            post_process_enabled: true,
            ..Default::default()
        };
        // Missing api_key, api_url, model should fall back to noop because
        // post-processing is optional and STT output remains valid.
        let p = create_post_processor(&config);
        assert_eq!(p.process("hello").unwrap(), "hello");
    }

    #[test]
    fn test_enabled_complete_config_returns_llm_processor() {
        let config = AppConfig {
            post_process_enabled: true,
            post_process_api_key: Some("key".to_string()),
            post_process_api_url: Some("https://api.example.com/v1/chat/completions".to_string()),
            post_process_model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        };
        let p = create_post_processor(&config);
        // LlmPostProcessor passes empty text through without a network call.
        assert_eq!(p.process("").unwrap(), "");
    }
}
