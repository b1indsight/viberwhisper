mod llm;

use crate::core::config::{ApiAuth, ConfigKey, PostProcessSection, ValidationIssue};
use llm::{LlmPostProcessor, LlmSession};
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
    pub(crate) fn validate(
        endpoint: Option<&str>,
        auth: ApiAuth,
        model: Option<&str>,
        section: &PostProcessSection,
    ) -> Result<Self, Vec<ValidationIssue>> {
        if !section.enabled {
            return Ok(Self::Disabled);
        }

        let mut issues = Vec::new();
        let endpoint = match endpoint {
            Some(value) => match reqwest::Url::parse(value) {
                Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
                Ok(_) => {
                    issues.push(ValidationIssue::new(
                        ConfigKey::ApiPostProcessUrl,
                        "post_process.url_scheme",
                        "post-process URL must use http or https",
                    ));
                    None
                }
                Err(error) => {
                    issues.push(ValidationIssue::new(
                        ConfigKey::ApiPostProcessUrl,
                        "post_process.url_invalid",
                        format!("invalid post-process URL: {error}"),
                    ));
                    None
                }
            },
            None => {
                issues.push(ValidationIssue::new(
                    ConfigKey::ApiPostProcessUrl,
                    "post_process.url_missing",
                    "post-process URL is required when enabled",
                ));
                None
            }
        };
        let model = match model.filter(|value| !value.trim().is_empty()) {
            Some(model) => Some(model.to_string()),
            None => {
                issues.push(ValidationIssue::new(
                    ConfigKey::ApiPostProcessModel,
                    "post_process.model_missing",
                    "post-process model is required when enabled",
                ));
                None
            }
        };
        match (endpoint, model) {
            (Some(endpoint), Some(model)) if issues.is_empty() => Ok(Self::Llm(LlmConfig {
                endpoint,
                auth,
                model,
                prompt: section.prompt.clone(),
                temperature: section.temperature,
                preheat_enabled: section.preheat_enabled,
            })),
            _ => Err(issues),
        }
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
pub struct PostProcessor(PostProcessorKind);

enum PostProcessorKind {
    Noop,
    Llm(LlmPostProcessor),
}

impl PostProcessor {
    pub fn new(config: PostProcessConfig) -> Self {
        match config {
            PostProcessConfig::Disabled => Self(PostProcessorKind::Noop),
            PostProcessConfig::Llm(config) => match LlmPostProcessor::new(config) {
                Ok(processor) => Self(PostProcessorKind::Llm(processor)),
                Err(error) => {
                    warn!(error = %error, "Failed to create LLM post-processor, falling back to noop");
                    Self(PostProcessorKind::Noop)
                }
            },
        }
    }

    pub fn process(&self, text: &str) -> Result<String, PostProcessError> {
        match &self.0 {
            PostProcessorKind::Noop => Ok(text.to_string()),
            PostProcessorKind::Llm(processor) => processor.process(text),
        }
    }

    pub fn start_session(&self) -> PostProcessorSession {
        let session = match &self.0 {
            PostProcessorKind::Noop => PostProcessorSessionKind::Noop(NoopSession::default()),
            PostProcessorKind::Llm(processor) => {
                PostProcessorSessionKind::Llm(Box::new(processor.start_session()))
            }
        };
        PostProcessorSession(session)
    }
}

/// Incremental cleanup state for one recording session.
pub struct PostProcessorSession(PostProcessorSessionKind);

enum PostProcessorSessionKind {
    Noop(NoopSession),
    Llm(Box<LlmSession>),
}

impl PostProcessorSession {
    pub fn push_stable_chunk(&mut self, text: &str) {
        match &mut self.0 {
            PostProcessorSessionKind::Noop(session) => session.push_stable_chunk(text),
            PostProcessorSessionKind::Llm(session) => session.push_stable_chunk(text),
        }
    }

    pub fn finish(&mut self) -> Result<String, PostProcessError> {
        match &mut self.0 {
            PostProcessorSessionKind::Noop(session) => Ok(session.finish()),
            PostProcessorSessionKind::Llm(session) => session.finish(),
        }
    }
}

#[derive(Default)]
struct NoopSession {
    chunks: Vec<String>,
}

impl NoopSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> String {
        self.chunks.join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_process() {
        let p = PostProcessor::new(PostProcessConfig::Disabled);
        assert_eq!(p.process("hello").unwrap(), "hello");
        assert_eq!(p.process("").unwrap(), "");
    }

    #[test]
    fn test_noop_session() {
        let p = PostProcessor::new(PostProcessConfig::Disabled);
        let mut session = p.start_session();
        session.push_stable_chunk("hello");
        session.push_stable_chunk("world");
        assert_eq!(session.finish().unwrap(), "helloworld");
    }
}
