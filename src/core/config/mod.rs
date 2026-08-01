mod document;
mod fields;
mod store;

use std::fmt;
use std::path::PathBuf;

pub use document::ConfigDocument;
pub(crate) use document::{
    AudioSection, ChunkingSection, InferenceProfile, InputSection, LocalSection,
    PostProcessSection, SessionSection, TranscriptionSection,
};
pub use fields::ConfigKey;
#[cfg(test)]
use fields::{FieldError, FieldValue, SecretStatus};
pub use store::ConfigStore;

#[derive(Debug)]
pub enum ConfigError {
    ConfigDirectoryUnavailable,
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidDocument {
        path: PathBuf,
        source: serde_json::Error,
    },
    Serialize(serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigDirectoryUnavailable => {
                write!(
                    formatter,
                    "could not determine the platform configuration directory"
                )
            }
            Self::Io { path, source } => {
                write!(formatter, "failed to access {}: {source}", path.display())
            }
            Self::InvalidDocument { path, source } => {
                write!(
                    formatter,
                    "invalid configuration {}: {source}",
                    path.display()
                )
            }
            Self::Serialize(source) => {
                write!(formatter, "failed to serialize configuration: {source}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidDocument { source, .. } | Self::Serialize(source) => Some(source),
            Self::ConfigDirectoryUnavailable => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub key: ConfigKey,
    pub code: &'static str,
    pub message: String,
}

impl ValidationIssue {
    pub(crate) fn new(key: ConfigKey, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            key,
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{}: {}", issue.key.as_str(), issue.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationReport {}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiAuth {
    None,
    Bearer(SecretValue),
}

pub trait SecretSource {
    fn get(&self, name: &str) -> Option<String>;
}

pub struct EnvironmentSecretSource;

impl SecretSource for EnvironmentSecretSource {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapSecrets(HashMap<&'static str, &'static str>);

    impl SecretSource for MapSecrets {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|value| (*value).to_string())
        }
    }

    #[test]
    fn canonical_example_round_trips_as_v2() {
        let document: ConfigDocument =
            serde_json::from_str(include_str!("../../../config.example.json")).unwrap();
        assert_eq!(document.schema_version, 2);
        let encoded = serde_json::to_string(&document).unwrap();
        assert_eq!(
            serde_json::from_str::<ConfigDocument>(&encoded).unwrap(),
            document
        );
    }

    #[test]
    fn rejects_non_finite_float_values_from_json() {
        let invalid = include_str!("../../../config.example.json")
            .replace("\"mic_gain\": 3.0", "\"mic_gain\": 1e100");
        assert!(serde_json::from_str::<ConfigDocument>(&invalid).is_err());
    }

    #[test]
    fn rejects_missing_wrong_and_flat_schema() {
        assert!(serde_json::from_str::<ConfigDocument>(r#"{"input": {}}"#).is_err());
        assert!(serde_json::from_str::<ConfigDocument>(r#"{"schema_version": 1}"#).is_err());
        assert!(
            serde_json::from_str::<ConfigDocument>(r#"{"schema_version": 2, "hold_hotkey": "F8"}"#)
                .is_err()
        );
    }

    #[test]
    fn store_uses_one_injected_path_for_defaults_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("config.json");
        let store = ConfigStore::at(path.clone());
        assert_eq!(store.path(), path.as_path());
        assert_eq!(store.load().unwrap(), ConfigDocument::default());
        assert!(!path.parent().unwrap().exists());

        let document: ConfigDocument =
            serde_json::from_str(include_str!("../../../config.example.json")).unwrap();
        store.save(&document).unwrap();
        assert_eq!(store.load().unwrap(), document);

        let defaults = ConfigDocument::default();
        store.save(&defaults).unwrap();
        assert_eq!(store.load().unwrap(), defaults);
    }

    #[test]
    fn existing_invalid_document_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(
            ConfigStore::at(path).load().unwrap_err(),
            ConfigError::InvalidDocument { .. }
        ));
    }

    #[test]
    fn field_catalog_uses_canonical_keys_and_rejects_legacy_aliases() {
        let keys: Vec<_> = ConfigDocument::field_keys()
            .iter()
            .map(|key| key.as_str())
            .collect();
        assert!(keys.contains(&"input.hold_hotkey"));
        assert!(keys.contains(&"inference.api.transcription.api_url"));
        assert!(keys.contains(&"inference.local.server_port"));
        assert!(!keys.contains(&"hold_hotkey"));
        assert!(!keys.contains(&"local_mode"));
        assert_eq!(keys.len(), 26);

        let document = ConfigDocument::default();
        let secrets = MapSecrets(HashMap::new());
        for key in ConfigDocument::field_keys() {
            document.get_field(key.as_str(), &secrets).unwrap();
        }
    }

    #[test]
    fn field_get_set_distinguishes_unknown_unset_and_read_only() {
        let mut document = ConfigDocument::default();
        let secrets = MapSecrets(HashMap::new());
        assert_eq!(
            document
                .get_field("inference.api.post_process.model", &secrets)
                .unwrap(),
            FieldValue::Unset
        );
        assert!(matches!(
            document.get_field("model", &secrets),
            Err(FieldError::UnknownKey(_))
        ));
        assert!(matches!(
            document.set_field("schema_version", "3"),
            Err(FieldError::ReadOnly(_))
        ));
        assert!(matches!(
            document.set_field("inference.api.transcription.api_key", "secret"),
            Err(FieldError::ReadOnly(_))
        ));
        document.set_field("audio.mic_gain", "2.5").unwrap();
        assert_eq!(
            document.get_field("audio.mic_gain", &secrets).unwrap(),
            FieldValue::Value("2.5".to_string())
        );
        assert!(document.set_field("audio.mic_gain", "NaN").is_err());
    }

    #[test]
    fn secret_status_reports_disk_environment_and_override_without_values() {
        let mut document = ConfigDocument::default();
        let none = MapSecrets(HashMap::new());
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key", &none)
                .unwrap(),
            FieldValue::Secret(SecretStatus::Unset)
        );
        document.inference.api.transcription.api_key = Some("disk-token".to_string());
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key", &none)
                .unwrap(),
            FieldValue::Secret(SecretStatus::Disk)
        );
        let environment = MapSecrets(HashMap::from([(
            "TRANSCRIPTION_API_KEY",
            "environment-token",
        )]));
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key", &environment)
                .unwrap(),
            FieldValue::Secret(SecretStatus::EnvironmentOverridesDisk)
        );
        document.inference.api.transcription.api_key = None;
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key", &environment)
                .unwrap(),
            FieldValue::Secret(SecretStatus::Environment)
        );
    }

    #[test]
    fn saving_never_persists_environment_secret_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        let environment = MapSecrets(HashMap::from([(
            "TRANSCRIPTION_API_KEY",
            "environment-token",
        )]));

        let mut document = ConfigDocument::default();
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key", &environment)
                .unwrap(),
            FieldValue::Secret(SecretStatus::Environment)
        );
        store.save(&document).unwrap();
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("environment-token")
        );

        document.inference.api.transcription.api_key = Some("disk-token".to_string());
        store.save(&document).unwrap();
        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(persisted.contains("disk-token"));
        assert!(!persisted.contains("environment-token"));
    }

    #[test]
    fn document_debug_redacts_disk_secrets() {
        let mut document = ConfigDocument::default();
        document.inference.api.transcription.api_key = Some("transcription-token".to_string());
        document.inference.api.post_process.api_key = Some("post-process-token".to_string());

        let debug = format!("{document:?}");
        assert!(!debug.contains("transcription-token"));
        assert!(!debug.contains("post-process-token"));
        assert!(debug.contains("[REDACTED]"));
    }
}
