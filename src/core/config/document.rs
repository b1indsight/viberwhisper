use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{EnvironmentSecretSource, SecretSource};

/// Persisted configuration with a lazily consulted secret source, shared by clones.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub(super) schema_version: u32,
    pub(crate) input: InputSection,
    pub(crate) audio: AudioSection,
    pub(crate) transcription: TranscriptionSection,
    pub(crate) post_process: PostProcessSection,
    pub(crate) inference: InferenceSection,
    #[serde(default)]
    pub(crate) dictation: crate::dictation::config::DictationConfig,
    #[serde(skip, default = "default_secrets")]
    pub(super) secrets: Arc<dyn SecretSource>,
}

fn default_secrets() -> Arc<dyn SecretSource> {
    Arc::new(EnvironmentSecretSource)
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self::new(EnvironmentSecretSource)
    }
}

impl ConfigDocument {
    /// Copies version-owned prompt fields from the embedded example into a candidate.
    /// Recognition composes only these saved components, never another default source.
    pub(crate) fn apply_bundled_prompts(&mut self) {
        let defaults = Self::default();
        self.transcription.prompt = defaults.transcription.prompt;
        self.post_process.prompt = defaults.post_process.prompt;
        self.dictation.prompt = defaults.dictation.prompt;
        self.dictation.prompt_components = defaults.dictation.prompt_components;
        self.dictation.legacy_short_prompt = None;
    }

    pub(crate) fn dictation_key(&self) -> Option<super::SecretValue> {
        if self.dictation.api_key_env.is_empty() {
            return None;
        }
        self.secrets
            .get(&self.dictation.api_key_env)
            .filter(|key| !key.is_empty())
            .map(super::SecretValue::new)
    }
    /// Creates configuration from the embedded example without reading credentials.
    /// The supplied lazy secret source replaces the example's environment-backed source.
    pub fn new(secrets: impl SecretSource + 'static) -> Self {
        let mut document: Self = serde_json::from_str(include_str!("../../../config.example.json"))
            .expect("bundled example configuration must be valid");
        document.secrets = Arc::new(secrets);
        document
    }

    pub(crate) fn set_transcription_api_key(&mut self, value: Option<String>) {
        self.inference.api.transcription.api_key = value;
    }

    pub(crate) fn set_post_process_api_key(&mut self, value: Option<String>) {
        self.inference.api.post_process.api_key = value;
    }
}

impl fmt::Debug for ConfigDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigDocument")
            .field("schema_version", &self.schema_version)
            .field("dictation", &self.dictation)
            .field("input", &self.input)
            .field("audio", &self.audio)
            .field("transcription", &self.transcription)
            .field("post_process", &self.post_process)
            .field("inference", &self.inference)
            .finish_non_exhaustive()
    }
}

// Equality describes the persisted document, independently of its runtime source.
impl PartialEq for ConfigDocument {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.input == other.input
            && self.audio == other.audio
            && self.transcription == other.transcription
            && self.post_process == other.post_process
            && self.inference == other.inference
            && self.dictation == other.dictation
    }
}

fn deserialize_schema_version<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == 3 {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported schema_version {version}; expected 3"
        )))
    }
}

fn deserialize_finite_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() && value.abs() <= f32::MAX as f64 {
        Ok(value as f32)
    } else {
        Err(serde::de::Error::custom(
            "value must be finite and within the f32 range",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputSection {
    pub(crate) hold_hotkey: String,
    pub(crate) toggle_hotkey: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioSection {
    #[serde(default)]
    pub(crate) input_device: Option<String>,
    #[serde(deserialize_with = "deserialize_finite_f32")]
    pub(crate) mic_gain: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TranscriptionSection {
    pub(crate) language: Option<String>,
    pub(crate) prompt: Option<String>,
    #[serde(deserialize_with = "deserialize_finite_f32")]
    pub(crate) temperature: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostProcessSection {
    pub(crate) enabled: bool,
    pub(crate) preheat_enabled: bool,
    pub(crate) prompt: Option<String>,
    #[serde(deserialize_with = "deserialize_finite_f32")]
    pub(crate) temperature: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InferenceSection {
    pub(crate) api: ApiInferenceSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiInferenceSection {
    pub(crate) transcription: ApiTranscriptionSection,
    pub(crate) post_process: ApiPostProcessSection,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiTranscriptionSection {
    pub(crate) api_url: String,
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,
}

impl fmt::Debug for ApiTranscriptionSection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiTranscriptionSection")
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiPostProcessSection {
    pub(crate) api_url: Option<String>,
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,
}

impl fmt::Debug for ApiPostProcessSection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiPostProcessSection")
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}
