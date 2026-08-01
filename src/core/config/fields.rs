use std::fmt;

use super::{ConfigDocument, InferenceProfile, SecretSource};

macro_rules! define_config_fields {
    ($(
        $variant:ident => {
            name: $name:literal,
            writable: $writable:literal
        }
    ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ConfigKey {
            $($variant),+
        }

        impl ConfigKey {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }

            fn parse(value: &str) -> Option<Self> {
                match value {
                    $($name => Some(Self::$variant)),+,
                    _ => None,
                }
            }

            fn is_writable(self) -> bool {
                match self {
                    $(Self::$variant => $writable),+
                }
            }
        }

        const CONFIG_KEYS: &[ConfigKey] = &[
            $(ConfigKey::$variant),+
        ];
    };
}

define_config_fields! {
    SchemaVersion => { name: "schema_version", writable: false },
    InputHoldHotkey => { name: "input.hold_hotkey", writable: true },
    InputToggleHotkey => { name: "input.toggle_hotkey", writable: true },
    AudioMicGain => { name: "audio.mic_gain", writable: true },
    ChunkingMaxDurationSecs => { name: "chunking.max_duration_secs", writable: true },
    ChunkingMaxSizeBytes => { name: "chunking.max_size_bytes", writable: true },
    ChunkingMaxRetries => { name: "chunking.max_retries", writable: true },
    SessionConvergenceTimeoutSecs => { name: "session.convergence_timeout_secs", writable: true },
    TranscriptionLanguage => { name: "transcription.language", writable: true },
    TranscriptionPrompt => { name: "transcription.prompt", writable: true },
    TranscriptionTemperature => { name: "transcription.temperature", writable: true },
    PostProcessEnabled => { name: "post_process.enabled", writable: true },
    PostProcessPreheatEnabled => { name: "post_process.preheat_enabled", writable: true },
    PostProcessPrompt => { name: "post_process.prompt", writable: true },
    PostProcessTemperature => { name: "post_process.temperature", writable: true },
    InferenceActive => { name: "inference.active", writable: true },
    ApiProvider => { name: "inference.api.provider", writable: true },
    ApiTranscriptionUrl => { name: "inference.api.transcription.api_url", writable: true },
    ApiTranscriptionModel => { name: "inference.api.transcription.model", writable: true },
    ApiTranscriptionKey => { name: "inference.api.transcription.api_key", writable: false },
    ApiPostProcessUrl => { name: "inference.api.post_process.api_url", writable: true },
    ApiPostProcessModel => { name: "inference.api.post_process.model", writable: true },
    ApiPostProcessKey => { name: "inference.api.post_process.api_key", writable: false },
    LocalDataDir => { name: "inference.local.data_dir", writable: true },
    LocalServerPort => { name: "inference.local.server_port", writable: true },
    LocalQuantization => { name: "inference.local.quantization", writable: true },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Value(String),
    Unset,
    Secret(SecretStatus),
}

impl fmt::Display for FieldValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => formatter.write_str(value),
            Self::Unset => formatter.write_str("(not set)"),
            Self::Secret(status) => write!(formatter, "{status}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStatus {
    Unset,
    Disk,
    Environment,
    EnvironmentOverridesDisk,
}

impl fmt::Display for SecretStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unset => "unset",
            Self::Disk => "set (disk)",
            Self::Environment => "set (environment)",
            Self::EnvironmentOverridesDisk => "set (environment overrides disk)",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldError {
    UnknownKey(String),
    ReadOnly(ConfigKey),
    InvalidValue { key: ConfigKey, message: String },
}

impl fmt::Display for FieldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(formatter, "unknown config key: {key}"),
            Self::ReadOnly(key) => write!(formatter, "{} is read-only", key.as_str()),
            Self::InvalidValue { key, message } => {
                write!(formatter, "invalid value for {}: {message}", key.as_str())
            }
        }
    }
}

impl std::error::Error for FieldError {}

impl ConfigDocument {
    pub fn field_keys() -> &'static [ConfigKey] {
        CONFIG_KEYS
    }

    pub fn get_field(
        &self,
        name: &str,
        secrets: &dyn SecretSource,
    ) -> Result<FieldValue, FieldError> {
        let key = ConfigKey::parse(name).ok_or_else(|| FieldError::UnknownKey(name.to_string()))?;
        if key == ConfigKey::ApiTranscriptionKey {
            return Ok(FieldValue::Secret(secret_status(
                self.inference.api.transcription.api_key.as_deref(),
                secrets.get("TRANSCRIPTION_API_KEY").as_deref(),
            )));
        }
        if key == ConfigKey::ApiPostProcessKey {
            return Ok(FieldValue::Secret(secret_status(
                self.inference.api.post_process.api_key.as_deref(),
                secrets.get("POST_PROCESS_API_KEY").as_deref(),
            )));
        }

        Ok(match key {
            ConfigKey::SchemaVersion => FieldValue::Value(self.schema_version.to_string()),
            ConfigKey::InputHoldHotkey => FieldValue::Value(self.input.hold_hotkey.clone()),
            ConfigKey::InputToggleHotkey => FieldValue::Value(self.input.toggle_hotkey.clone()),
            ConfigKey::AudioMicGain => FieldValue::Value(self.audio.mic_gain.to_string()),
            ConfigKey::ChunkingMaxDurationSecs => {
                FieldValue::Value(self.chunking.max_duration_secs.to_string())
            }
            ConfigKey::ChunkingMaxSizeBytes => {
                FieldValue::Value(self.chunking.max_size_bytes.to_string())
            }
            ConfigKey::ChunkingMaxRetries => {
                FieldValue::Value(self.chunking.max_retries.to_string())
            }
            ConfigKey::SessionConvergenceTimeoutSecs => {
                FieldValue::Value(self.session.convergence_timeout_secs.to_string())
            }
            ConfigKey::TranscriptionLanguage => optional_field(&self.transcription.language),
            ConfigKey::TranscriptionPrompt => optional_field(&self.transcription.prompt),
            ConfigKey::TranscriptionTemperature => {
                FieldValue::Value(self.transcription.temperature.to_string())
            }
            ConfigKey::PostProcessEnabled => {
                FieldValue::Value(self.post_process.enabled.to_string())
            }
            ConfigKey::PostProcessPreheatEnabled => {
                FieldValue::Value(self.post_process.preheat_enabled.to_string())
            }
            ConfigKey::PostProcessPrompt => optional_field(&self.post_process.prompt),
            ConfigKey::PostProcessTemperature => {
                FieldValue::Value(self.post_process.temperature.to_string())
            }
            ConfigKey::InferenceActive => FieldValue::Value(
                match self.inference.active {
                    InferenceProfile::Api => "api",
                    InferenceProfile::Local => "local",
                }
                .to_string(),
            ),
            ConfigKey::ApiProvider => optional_field(&self.inference.api.provider),
            ConfigKey::ApiTranscriptionUrl => {
                FieldValue::Value(self.inference.api.transcription.api_url.clone())
            }
            ConfigKey::ApiTranscriptionModel => {
                FieldValue::Value(self.inference.api.transcription.model.clone())
            }
            ConfigKey::ApiPostProcessUrl => {
                optional_field(&self.inference.api.post_process.api_url)
            }
            ConfigKey::ApiPostProcessModel => {
                optional_field(&self.inference.api.post_process.model)
            }
            ConfigKey::LocalDataDir => optional_field(&self.inference.local.data_dir),
            ConfigKey::LocalServerPort => {
                FieldValue::Value(self.inference.local.server_port.to_string())
            }
            ConfigKey::LocalQuantization => {
                FieldValue::Value(self.inference.local.quantization.clone())
            }
            ConfigKey::ApiTranscriptionKey | ConfigKey::ApiPostProcessKey => unreachable!(),
        })
    }

    pub fn set_field(&mut self, name: &str, value: &str) -> Result<(), FieldError> {
        let key = ConfigKey::parse(name).ok_or_else(|| FieldError::UnknownKey(name.to_string()))?;
        if !key.is_writable() {
            return Err(FieldError::ReadOnly(key));
        }

        let invalid = |message: String| FieldError::InvalidValue { key, message };
        match key {
            ConfigKey::InputHoldHotkey => self.input.hold_hotkey = value.to_string(),
            ConfigKey::InputToggleHotkey => self.input.toggle_hotkey = value.to_string(),
            ConfigKey::AudioMicGain => {
                self.audio.mic_gain = parse_f32(value).map_err(invalid)?;
            }
            ConfigKey::ChunkingMaxDurationSecs => {
                self.chunking.max_duration_secs = parse_number::<u32>(value).map_err(invalid)?;
            }
            ConfigKey::ChunkingMaxSizeBytes => {
                self.chunking.max_size_bytes = parse_number::<u64>(value).map_err(invalid)?;
            }
            ConfigKey::ChunkingMaxRetries => {
                self.chunking.max_retries = parse_number::<u32>(value).map_err(invalid)?;
            }
            ConfigKey::SessionConvergenceTimeoutSecs => {
                self.session.convergence_timeout_secs =
                    parse_number::<u64>(value).map_err(invalid)?;
            }
            ConfigKey::TranscriptionLanguage => self.transcription.language = parse_optional(value),
            ConfigKey::TranscriptionPrompt => self.transcription.prompt = parse_optional(value),
            ConfigKey::TranscriptionTemperature => {
                self.transcription.temperature = parse_f32(value).map_err(invalid)?;
            }
            ConfigKey::PostProcessEnabled => {
                self.post_process.enabled = parse_number::<bool>(value).map_err(invalid)?;
            }
            ConfigKey::PostProcessPreheatEnabled => {
                self.post_process.preheat_enabled = parse_number::<bool>(value).map_err(invalid)?;
            }
            ConfigKey::PostProcessPrompt => self.post_process.prompt = parse_optional(value),
            ConfigKey::PostProcessTemperature => {
                self.post_process.temperature = parse_f32(value).map_err(invalid)?;
            }
            ConfigKey::InferenceActive => {
                self.inference.active = match value.to_ascii_lowercase().as_str() {
                    "api" => InferenceProfile::Api,
                    "local" => InferenceProfile::Local,
                    _ => return Err(invalid("expected `api` or `local`".to_string())),
                };
            }
            ConfigKey::ApiProvider => self.inference.api.provider = parse_optional(value),
            ConfigKey::ApiTranscriptionUrl => {
                self.inference.api.transcription.api_url = value.to_string()
            }
            ConfigKey::ApiTranscriptionModel => {
                self.inference.api.transcription.model = value.to_string()
            }
            ConfigKey::ApiPostProcessUrl => {
                self.inference.api.post_process.api_url = parse_optional(value)
            }
            ConfigKey::ApiPostProcessModel => {
                self.inference.api.post_process.model = parse_optional(value)
            }
            ConfigKey::LocalDataDir => self.inference.local.data_dir = parse_optional(value),
            ConfigKey::LocalServerPort => {
                self.inference.local.server_port = parse_number::<u16>(value).map_err(invalid)?;
            }
            ConfigKey::LocalQuantization => self.inference.local.quantization = value.to_string(),
            ConfigKey::SchemaVersion
            | ConfigKey::ApiTranscriptionKey
            | ConfigKey::ApiPostProcessKey => unreachable!(),
        }
        Ok(())
    }
}

fn optional_field(value: &Option<String>) -> FieldValue {
    value
        .as_ref()
        .map_or(FieldValue::Unset, |value| FieldValue::Value(value.clone()))
}

fn parse_optional(value: &str) -> Option<String> {
    (!value.eq_ignore_ascii_case("null")).then(|| value.to_string())
}

fn parse_f32(value: &str) -> Result<f32, String> {
    let parsed = value.parse::<f32>().map_err(|error| error.to_string())?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err("value must be finite".to_string())
    }
}

fn parse_number<T>(value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    value.parse::<T>().map_err(|error| error.to_string())
}

fn secret_status(disk: Option<&str>, environment: Option<&str>) -> SecretStatus {
    match (
        disk.filter(|value| !value.is_empty()),
        environment.filter(|value| !value.is_empty()),
    ) {
        (None, None) => SecretStatus::Unset,
        (Some(_), None) => SecretStatus::Disk,
        (None, Some(_)) => SecretStatus::Environment,
        (Some(_), Some(_)) => SecretStatus::EnvironmentOverridesDisk,
    }
}
