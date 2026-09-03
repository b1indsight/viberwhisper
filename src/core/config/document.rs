use std::fmt;

use serde::{Deserialize, Serialize};

const DEFAULT_TRANSCRIPTION_API_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub(super) schema_version: u32,
    pub(crate) input: InputSection,
    pub(crate) audio: AudioSection,
    pub(crate) transcription: TranscriptionSection,
    pub(crate) post_process: PostProcessSection,
    pub(crate) inference: InferenceSection,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            schema_version: 2,
            input: InputSection::default(),
            audio: AudioSection::default(),
            transcription: TranscriptionSection::default(),
            post_process: PostProcessSection::default(),
            inference: InferenceSection::default(),
        }
    }
}

impl ConfigDocument {
    pub(crate) fn set_transcription_api_key(&mut self, value: Option<String>) {
        self.inference.api.transcription.api_key = value;
    }

    pub(crate) fn set_post_process_api_key(&mut self, value: Option<String>) {
        self.inference.api.post_process.api_key = value;
    }
}

fn deserialize_schema_version<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == 2 {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported schema_version {version}; expected 2"
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

impl Default for InputSection {
    fn default() -> Self {
        Self {
            hold_hotkey: "F8".to_string(),
            toggle_hotkey: "F9".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioSection {
    #[serde(default)]
    pub(crate) input_device: Option<String>,
    #[serde(deserialize_with = "deserialize_finite_f32")]
    pub(crate) mic_gain: f32,
}

impl Default for AudioSection {
    fn default() -> Self {
        Self {
            input_device: None,
            mic_gain: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TranscriptionSection {
    pub(crate) language: Option<String>,
    pub(crate) prompt: Option<String>,
    #[serde(deserialize_with = "deserialize_finite_f32")]
    pub(crate) temperature: f32,
}

impl Default for TranscriptionSection {
    fn default() -> Self {
        Self {
            language: Some("zh".to_string()),
            prompt: Some("以下是一段简体中文的普通话句子，去掉首尾的语气词".to_string()),
            temperature: 0.0,
        }
    }
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

impl Default for PostProcessSection {
    fn default() -> Self {
        Self {
            enabled: false,
            preheat_enabled: true,
            prompt: None,
            temperature: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InferenceProfile {
    Api,
    Local,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InferenceSection {
    pub(crate) active: InferenceProfile,
    pub(crate) api: ApiInferenceSection,
    pub(crate) local: LocalSection,
}

impl Default for InferenceSection {
    fn default() -> Self {
        Self {
            active: InferenceProfile::Api,
            api: ApiInferenceSection::default(),
            local: LocalSection::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

impl Default for ApiTranscriptionSection {
    fn default() -> Self {
        Self {
            api_url: DEFAULT_TRANSCRIPTION_API_URL.to_string(),
            model: "whisper-large-v3-turbo".to_string(),
            api_key: None,
        }
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalSection {
    pub(crate) data_dir: Option<String>,
    pub(crate) server_port: u16,
    pub(crate) quantization: String,
}

impl Default for LocalSection {
    fn default() -> Self {
        Self {
            data_dir: Some("~/.viberwhisper".to_string()),
            server_port: 17_265,
            quantization: "int8".to_string(),
        }
    }
}
