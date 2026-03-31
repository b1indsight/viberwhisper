use crate::core::config::AppConfig;
use crate::postprocess::{LlmPostProcessor, NoopPostProcessor, TextPostProcessor};
use tracing::warn;

/// Create a post-processor from config.
///
/// Returns `NoopPostProcessor` if post-processing is disabled or misconfigured,
/// ensuring the main pipeline is never blocked by a missing LLM setup.
pub fn create_post_processor(config: &AppConfig) -> Box<dyn TextPostProcessor> {
    if !config.post_process_enabled {
        return Box::new(NoopPostProcessor);
    }

    match LlmPostProcessor::from_config(config) {
        Ok(processor) => Box::new(processor),
        Err(e) => {
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
        let mut config = AppConfig::default();
        config.post_process_enabled = true;
        // Missing api_key, api_url, model — should silently fall back to noop.
        let p = create_post_processor(&config);
        assert_eq!(p.process("hello").unwrap(), "hello");
    }

    #[test]
    fn test_enabled_complete_config_returns_llm_processor() {
        let mut config = AppConfig::default();
        config.post_process_enabled = true;
        config.post_process_api_key = Some("key".to_string());
        config.post_process_api_url =
            Some("https://api.example.com/v1/chat/completions".to_string());
        config.post_process_model = Some("gpt-4o-mini".to_string());
        let p = create_post_processor(&config);
        // LlmPostProcessor passes empty text through without a network call.
        assert_eq!(p.process("").unwrap(), "");
    }
}
