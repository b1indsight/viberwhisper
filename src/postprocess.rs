mod llm;
#[cfg(test)]
pub(crate) mod test_support;

use crate::core::config::{ApiAuth, ConfigDocument, fields};
use crate::dictation::{LearningWorker, Transcript, config::MemoryConfig};
use anyhow::Context;
pub use llm::LlmPostProcessor;
use std::fmt;
use tracing::warn;

#[derive(Debug)]
pub struct LlmConfig {
    endpoint: reqwest::Url,
    auth: ApiAuth,
    model: String,
    prompt: Option<String>,
    temperature: f32,
    preheat_enabled: bool,
}

#[derive(Debug)]
pub enum PostProcessConfig {
    Disabled,
    Llm(LlmConfig),
}

impl PostProcessConfig {
    /// Resolves enabled post-processing without consulting unused LLM fields when disabled.
    pub(crate) fn from_config(document: &ConfigDocument) -> anyhow::Result<Self> {
        if !document.select(fields::PostProcessEnabled, std::convert::identity) {
            return Ok(Self::Disabled);
        }
        document.select(
            (
                fields::ApiPostProcessUrl,
                fields::ApiPostProcessKey,
                fields::ApiPostProcessModel,
                fields::PostProcessPrompt,
                fields::PostProcessTemperature,
                fields::PostProcessPreheatEnabled,
            ),
            |(endpoint, key, model, prompt, temperature, preheat_enabled)| {
                let endpoint = endpoint.context(
                    "inference.api.post_process.api_url is required when cleanup is enabled",
                )?;
                Ok(Self::Llm(LlmConfig {
                    endpoint: reqwest::Url::parse(&endpoint)
                        .context("invalid inference.api.post_process.api_url")?,
                    auth: key.auth,
                    model: model.context(
                        "inference.api.post_process.model is required when cleanup is enabled",
                    )?,
                    prompt,
                    temperature,
                    preheat_enabled,
                }))
            },
        )
    }
}

#[derive(Debug)]
pub enum PostProcessError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    InvalidResult,
    Memory(anyhow::Error),
    Api { status: u16 },
    MissingContent,
    EmptyContent,
}

impl fmt::Display for PostProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "LLM request failed: {error}"),
            Self::Json(error) => write!(f, "invalid LLM response: {error}"),
            Self::InvalidResult => write!(f, "invalid structured cleanup result"),
            Self::Memory(error) => write!(f, "cleanup memory unavailable: {error}"),
            // Providers may echo the transcript or context in their error body.
            Self::Api { status } => write!(f, "LLM API error {status}"),
            Self::MissingContent => write!(f, "content field not found in LLM response"),
            Self::EmptyContent => write!(f, "LLM returned empty content"),
        }
    }
}

impl std::error::Error for PostProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Memory(error) => Some(error.as_ref()),
            Self::InvalidResult | Self::Api { .. } | Self::MissingContent | Self::EmptyContent => {
                None
            }
        }
    }
}

impl From<reqwest::Error> for PostProcessError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

impl From<serde_json::Error> for PostProcessError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Configured text cleanup implementation.
///
/// LLM client construction falls back to pass-through behavior because cleanup
/// is optional and must not make speech-to-text unavailable.
pub enum PostProcessor {
    Disabled,
    Llm(Box<LlmPostProcessor>),
}

impl PostProcessor {
    /// Desktop-only memory ownership; disabled cleanup never loads personal memory.
    pub(crate) fn new_desktop(
        config: PostProcessConfig,
        memory: MemoryConfig,
        path: Option<std::path::PathBuf>,
        language: Option<String>,
    ) -> anyhow::Result<Self> {
        let mut processor = Self::new(config);
        if let Self::Llm(processor) = &mut processor
            && memory.enabled
        {
            processor.learning = Some(LearningWorker::new(memory, path, language)?);
        }
        Ok(processor)
    }

    /// Creates a processor from resolved runtime settings.
    pub fn new(config: PostProcessConfig) -> Self {
        match config {
            PostProcessConfig::Disabled => Self::Disabled,
            PostProcessConfig::Llm(config) => match LlmPostProcessor::new(config) {
                Ok(processor) => Self::Llm(Box::new(processor)),
                Err(error) => {
                    warn!(error = %error, "Failed to create LLM post-processor, falling back to noop");
                    Self::Disabled
                }
            },
        }
    }

    /// Processes a complete text input in one call.
    pub fn process_text(&self, text: &str) -> Result<String, PostProcessError> {
        match self {
            Self::Disabled => Ok(text.to_string()),
            Self::Llm(processor) => processor.process_text(text),
        }
    }

    /// Preserve validated recognition metadata until the user successfully delivers the result.
    pub(crate) fn process_for_delivery(&self, text: &str) -> Result<Transcript, PostProcessError> {
        match self {
            Self::Disabled => Ok(text.to_owned().into()),
            Self::Llm(processor) => processor.process_for_delivery(text),
        }
    }

    /// Drain accepted feedback without waiting for an unconfirmed preview or HTTP request.
    pub(crate) fn shutdown(&self) {
        if let Self::Llm(processor) = self
            && let Some(learning) = &processor.learning
        {
            learning.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::SecretSource;

    #[test]
    fn test_noop_process_text() {
        let p = PostProcessor::new(PostProcessConfig::Disabled);
        assert_eq!(p.process_text("hello").unwrap(), "hello");
        assert_eq!(p.process_text("").unwrap(), "");
    }

    #[test]
    fn disabled_delivery_has_no_confirmation_feedback() {
        let p = PostProcessor::new(PostProcessConfig::Disabled);
        let result = p.process_for_delivery("hello world").unwrap();
        assert_eq!(result.text, "hello world");
        assert!(result.confirmation.is_none());
    }

    #[test]
    fn disabled_config_request_skips_llm_fields_and_secrets() {
        struct NoSecrets;
        impl SecretSource for NoSecrets {
            fn get(&self, _name: &str) -> Option<String> {
                panic!("disabled cleanup must not resolve API credentials");
            }
        }
        let mut document = ConfigDocument::new(NoSecrets);
        document.post_process.enabled = false;
        document.inference.api.post_process.api_url = Some("not a URL".to_string());
        // Turning cleanup off must bypass stale endpoint settings and secret providers.
        assert!(matches!(
            PostProcessConfig::from_config(&document),
            Ok(PostProcessConfig::Disabled)
        ));
    }
}
