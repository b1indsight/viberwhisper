//! Canonical field names, typed selections, and redacted CLI field access.

use std::fmt;

use super::{ApiAuth, ConfigDocument, SecretValue};

/// A typed field selection. Tuples combine selections without reading other fields.
pub trait FieldRequest {
    type Values;

    fn read(self, document: &ConfigDocument) -> Self::Values;
}

macro_rules! field_tuple {
    ($($kind:ident: $value:ident),+) => {
        impl<$($kind: FieldRequest),+> FieldRequest for ($($kind,)+) {
            type Values = ($($kind::Values,)+);

            fn read(self, document: &ConfigDocument) -> Self::Values {
                let ($($value,)+) = self;
                ($($value.read(document),)+)
            }
        }
    };
}

field_tuple!(A: a, B: b);
field_tuple!(A: a, B: b, C: c);
field_tuple!(A: a, B: b, C: c, D: d);
field_tuple!(A: a, B: b, C: c, D: d, E: e);
field_tuple!(A: a, B: b, C: c, D: d, E: e, F: f);

macro_rules! define_config_fields {
    ($(
        $variant:ident => {
            name: $name:literal,
            writable: $writable:literal,
            value: $value:ty,
            read: $read:expr
        }
    ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ConfigKey {
            $($variant),+
        }

        $(
            #[doc = concat!("Selects `", $name, "` with its native value type.")]
            #[derive(Debug, Clone, Copy)]
            pub struct $variant;

            impl FieldRequest for $variant {
                type Values = $value;

                fn read(self, document: &ConfigDocument) -> Self::Values {
                    let read: fn(&ConfigDocument) -> $value = $read;
                    read(document)
                }
            }
        )+

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

            fn value(self, document: &ConfigDocument) -> FieldValue {
                match self {
                    $(Self::$variant => $variant.read(document).into()),+
                }
            }
        }

        const CONFIG_KEYS: &[ConfigKey] = &[$(ConfigKey::$variant),+];
    };
}

define_config_fields! {
    SchemaVersion => {
        name: "schema_version", writable: false, value: u32,
        read: |document| document.schema_version
    },
    InputHoldHotkey => {
        name: "input.hold_hotkey", writable: true, value: String,
        read: |document| document.input.hold_hotkey.clone()
    },
    InputToggleHotkey => {
        name: "input.toggle_hotkey", writable: true, value: String,
        read: |document| document.input.toggle_hotkey.clone()
    },
    AudioInputDevice => {
        name: "audio.input_device", writable: true, value: Option<String>,
        read: |document| document.audio.input_device.clone()
    },
    AudioMicGain => {
        name: "audio.mic_gain", writable: true, value: f32,
        read: |document| document.audio.mic_gain
    },
    TranscriptionLanguage => {
        name: "transcription.language", writable: true, value: Option<String>,
        read: |document| document.transcription.language.clone()
    },
    TranscriptionPrompt => {
        name: "transcription.prompt", writable: true, value: Option<String>,
        read: |document| document.transcription.prompt.clone()
    },
    TranscriptionTemperature => {
        name: "transcription.temperature", writable: true, value: f32,
        read: |document| document.transcription.temperature
    },
    PostProcessEnabled => {
        name: "post_process.enabled", writable: true, value: bool,
        read: |document| document.post_process.enabled
    },
    PostProcessPreheatEnabled => {
        name: "post_process.preheat_enabled", writable: true, value: bool,
        read: |document| document.post_process.preheat_enabled
    },
    PostProcessPrompt => {
        name: "post_process.prompt", writable: true, value: Option<String>,
        read: |document| document.post_process.prompt.clone()
    },
    PostProcessTemperature => {
        name: "post_process.temperature", writable: true, value: f32,
        read: |document| document.post_process.temperature
    },
    ApiTranscriptionUrl => {
        name: "inference.api.transcription.api_url", writable: true, value: String,
        read: |document| document.inference.api.transcription.api_url.clone()
    },
    ApiTranscriptionModel => {
        name: "inference.api.transcription.model", writable: true, value: String,
        read: |document| document.inference.api.transcription.model.clone()
    },
    ApiTranscriptionKey => {
        name: "inference.api.transcription.api_key", writable: false, value: ResolvedSecret,
        read: |document| document.resolve_secret("TRANSCRIPTION_API_KEY", document.inference.api.transcription.api_key.as_deref())
    },
    ApiPostProcessUrl => {
        name: "inference.api.post_process.api_url", writable: true, value: Option<String>,
        read: |document| document.inference.api.post_process.api_url.clone()
    },
    ApiPostProcessModel => {
        name: "inference.api.post_process.model", writable: true, value: Option<String>,
        read: |document| document.inference.api.post_process.model.clone()
    },
    ApiPostProcessKey => {
        name: "inference.api.post_process.api_key", writable: false, value: ResolvedSecret,
        read: |document| document.resolve_secret("POST_PROCESS_API_KEY", document.inference.api.post_process.api_key.as_deref())
    },
}

/// A resolved credential with a redacted CLI view of its source.
#[derive(Debug)]
pub struct ResolvedSecret {
    pub(crate) auth: ApiAuth,
    pub(crate) status: SecretStatus,
}

impl ConfigDocument {
    fn resolve_secret(&self, name: &str, disk: Option<&str>) -> ResolvedSecret {
        let environment = self.secrets.get(name);
        let status = secret_status(disk, environment.as_deref());
        let value = environment
            .filter(|value| !value.is_empty())
            .or_else(|| disk.filter(|value| !value.is_empty()).map(str::to_string));
        ResolvedSecret {
            auth: value.map_or(ApiAuth::None, |value| {
                ApiAuth::Bearer(SecretValue::new(value))
            }),
            status,
        }
    }
}

impl From<ResolvedSecret> for FieldValue {
    fn from(value: ResolvedSecret) -> Self {
        Self::Secret(value.status)
    }
}

impl From<String> for FieldValue {
    fn from(value: String) -> Self {
        Self::Value(value)
    }
}

impl From<Option<String>> for FieldValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(Self::Unset, Self::Value)
    }
}

macro_rules! display_field_values {
    ($($kind:ty),+) => {
        $(impl From<$kind> for FieldValue {
            fn from(value: $kind) -> Self {
                Self::Value(value.to_string())
            }
        })+
    };
}

display_field_values!(u32, f32, bool);

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
    /// Builds a caller-owned value from one typed field or a tuple of typed fields.
    ///
    /// Only requested fields and their bound secret source are read. The caller's
    /// constructor owns the output type and any construction errors.
    pub fn select<R: FieldRequest, T>(&self, fields: R, build: impl FnOnce(R::Values) -> T) -> T {
        build(fields.read(self))
    }

    pub fn field_keys() -> &'static [ConfigKey] {
        CONFIG_KEYS
    }

    pub fn get_field(&self, name: &str) -> Result<FieldValue, FieldError> {
        let key = ConfigKey::parse(name).ok_or_else(|| FieldError::UnknownKey(name.to_string()))?;
        Ok(key.value(self))
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
            ConfigKey::AudioInputDevice => self.audio.input_device = parse_optional(value),
            ConfigKey::AudioMicGain => {
                self.audio.mic_gain = parse_f32(value).map_err(invalid)?;
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
            ConfigKey::SchemaVersion
            | ConfigKey::ApiTranscriptionKey
            | ConfigKey::ApiPostProcessKey => unreachable!(),
        }
        Ok(())
    }
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
