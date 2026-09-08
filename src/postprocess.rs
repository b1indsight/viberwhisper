mod llm;

use crate::core::config::{ApiAuth, ConfigDocument, fields};
use anyhow::Context;
pub use llm::{ConservativeLlmSession, LlmPostProcessor, PreheatLlmSession};
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
    Api { status: u16, body: String },
    MissingContent,
    EmptyContent,
}

impl fmt::Display for PostProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "LLM request failed: {error}"),
            Self::Json(error) => write!(f, "invalid LLM response: {error}"),
            Self::Api { status, body } => write!(f, "LLM API error {status}: {body}"),
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
            Self::Api { .. } | Self::MissingContent | Self::EmptyContent => None,
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
    Llm(LlmPostProcessor),
}

impl PostProcessor {
    /// Creates a processor from resolved runtime settings.
    pub fn new(config: PostProcessConfig) -> Self {
        match config {
            PostProcessConfig::Disabled => Self::Disabled,
            PostProcessConfig::Llm(config) => match LlmPostProcessor::new(config) {
                Ok(processor) => Self::Llm(processor),
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

    /// Creates independent state for incremental text cleanup.
    pub fn create_session(&self) -> PostProcessorSession {
        match self {
            Self::Disabled => PostProcessorSession::Noop(NoopSession::default()),
            Self::Llm(processor) => processor.create_session(),
        }
    }
}

/// Incremental text cleanup state for one recording session.
///
/// Created by [`PostProcessor::create_session`]. Feed stable text chunks with
/// [`Self::push_stable_chunk`], then call [`Self::finish`] for the final text.
pub enum PostProcessorSession {
    Noop(NoopSession),
    Conservative(ConservativeLlmSession),
    Preheat(PreheatLlmSession),
}

impl PostProcessorSession {
    /// Adds a stable text fragment to the session's accumulated input.
    pub fn push_stable_chunk(&mut self, text: &str) {
        match self {
            Self::Noop(session) => session.push_stable_chunk(text),
            Self::Conservative(session) => session.push_stable_chunk(text),
            Self::Preheat(session) => session.push_stable_chunk(text),
        }
    }

    /// Completes cleanup and returns the processed text.
    pub fn finish(&mut self) -> Result<String, PostProcessError> {
        match self {
            Self::Noop(session) => session.finish(),
            Self::Conservative(session) => session.finish(),
            Self::Preheat(session) => session.finish(),
        }
    }
}

/// Pass-through state owned by [`PostProcessorSession::Noop`].
#[derive(Default)]
pub struct NoopSession {
    chunks: Vec<String>,
}

impl NoopSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> Result<String, PostProcessError> {
        Ok(self.chunks.join(""))
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
    fn test_noop_session() {
        let p = PostProcessor::new(PostProcessConfig::Disabled);
        let mut session = p.create_session();
        session.push_stable_chunk("hello");
        session.push_stable_chunk("world");
        assert_eq!(session.finish().unwrap(), "helloworld");
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
