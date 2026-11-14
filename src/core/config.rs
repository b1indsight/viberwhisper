//! Configuration persistence and typed field selection.
//!
//! Callers declare fields with `fields` selectors and construct their own values through
//! `ConfigDocument::select`. Configuration knows no business or application types.

mod document;
pub mod fields;
mod store;

use std::fmt;
use std::path::PathBuf;

pub use document::ConfigDocument;
pub(crate) use document::InputSection;
pub use fields::ConfigKey;
#[cfg(test)]
use fields::{FieldError, FieldValue, SecretStatus};
pub use store::ConfigStore;
pub(crate) use store::config_dir;

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

/// Lazy credential lookup bound to a document and shared when that document is cloned.
pub trait SecretSource: Send + Sync {
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
    fn canonical_example_round_trips_as_v3() {
        let document: ConfigDocument =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        // Fresh setup and upgrades must share the example's defaults, not a second
        // constructor that silently supplies different audio or prompt settings.
        assert_eq!(ConfigDocument::default(), document);
        assert_eq!(document.schema_version, 3);
        let encoded = serde_json::to_string(&document).unwrap();
        let encoded_value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert!(encoded_value.get("chunking").is_none());
        assert!(encoded_value.get("session").is_none());
        assert!(encoded_value["inference"]["api"].get("provider").is_none());
        assert!(encoded_value["inference"].get("active").is_none());
        assert!(encoded_value["inference"].get("local").is_none());
        assert_eq!(
            serde_json::from_str::<ConfigDocument>(&encoded).unwrap(),
            document
        );
    }

    #[test]
    fn retired_policy_fields_are_rejected() {
        // The current schema is the only accepted shape: removed policy knobs must not be
        // mistaken for live settings that still affect runtime behavior.
        let canonical: serde_json::Value =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();

        let mut with_chunking = canonical.clone();
        with_chunking["chunking"] = serde_json::json!({
            "max_duration_secs": 30,
            "max_size_bytes": 24117248,
            "max_retries": 1
        });
        assert!(serde_json::from_value::<ConfigDocument>(with_chunking).is_err());

        let mut with_session = canonical.clone();
        with_session["session"] = serde_json::json!({"convergence_timeout_secs": 30});
        assert!(serde_json::from_value::<ConfigDocument>(with_session).is_err());

        let mut with_provider = canonical;
        with_provider["inference"]["api"]["provider"] = serde_json::json!("groq");
        assert!(serde_json::from_value::<ConfigDocument>(with_provider).is_err());
    }

    #[test]
    fn rejects_non_finite_float_values_from_json() {
        let invalid = include_str!("../../config.example.json")
            .replace("\"mic_gain\": 3.0", "\"mic_gain\": 1e100");
        assert!(serde_json::from_str::<ConfigDocument>(&invalid).is_err());
    }

    #[test]
    fn rejects_invalid_document_shapes() {
        assert!(serde_json::from_str::<ConfigDocument>(r#"{"input": {}}"#).is_err());
        assert!(serde_json::from_str::<ConfigDocument>(r#"{"schema_version": 2}"#).is_err());
        assert!(
            serde_json::from_str::<ConfigDocument>(r#"{"schema_version": 3, "hold_hotkey": "F8"}"#)
                .is_err()
        );

        // Adding a skipped runtime field must not expand the strict on-disk schema.
        let mut value = serde_json::to_value(ConfigDocument::default()).unwrap();
        value["secrets"] = serde_json::json!({"TRANSCRIPTION_API_KEY": "unexpected"});
        assert!(serde_json::from_value::<ConfigDocument>(value).is_err());
    }

    #[test]
    fn store_uses_one_injected_path_for_missing_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("config.json");
        let store = ConfigStore::at(path.clone());
        assert_eq!(store.path(), path.as_path());
        assert_eq!(store.load().unwrap(), None);
        assert!(!path.parent().unwrap().exists());

        let document: ConfigDocument =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        store.save(&document).unwrap();
        assert_eq!(store.load().unwrap(), Some(document));

        let defaults = ConfigDocument::default();
        store.save(&defaults).unwrap();
        assert_eq!(store.load().unwrap(), Some(defaults));
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
    fn device_selection_preserves_latest_settings_and_rejects_unusable_documents() {
        // Tray selection must not overwrite edits made since startup or recreate a missing file.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        assert!(store.set_input_device(Some("USB Mic")).is_err());
        assert!(!path.exists());

        let mut document = ConfigDocument::default();
        store.save(&document).unwrap();
        store.set_input_device(Some("USB & 会议\nMic")).unwrap();
        document.audio.input_device = Some("USB & 会议\nMic".into());
        assert_eq!(store.load().unwrap(), Some(document.clone()));

        document.audio.mic_gain = 1.5;
        store.save(&document).unwrap();
        store.set_input_device(None).unwrap();
        document.audio.input_device = None;
        assert_eq!(store.load().unwrap(), Some(document));

        std::fs::write(&path, "invalid json").unwrap();
        assert!(store.set_input_device(Some("Other Mic")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid json");
    }

    #[test]
    fn gain_selection_preserves_latest_settings_and_rejects_unusable_documents() {
        // A tray edit must change only the gain, not restore the startup device or other settings.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        assert!(store.set_mic_gain(0.5).is_err());
        assert!(!path.exists());

        let mut document = ConfigDocument::default();
        store.save(&document).unwrap();
        document.audio.input_device = Some("USB 会议麦克风".into());
        document.transcription.prompt = Some("外部修改的提示词".into());
        store.save(&document).unwrap();
        store.set_mic_gain(0.5).unwrap();
        document.audio.mic_gain = 0.5;
        assert_eq!(store.load().unwrap(), Some(document));

        std::fs::write(&path, "invalid json").unwrap();
        assert!(store.set_mic_gain(2.0).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid json");
    }

    #[test]
    fn startup_replaces_old_prompts_on_disk_without_changing_other_settings() {
        // A complete old config must upgrade too: fill-missing alone leaves custom or
        // empty instructions masking the new version, even when other fields are valid.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        let bundled: serde_json::Value =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        for prompt in [
            serde_json::json!("旧规则"),
            serde_json::json!(""),
            serde_json::Value::Null,
            serde_json::json!("字".repeat(5000)),
        ] {
            let mut original = bundled.clone();
            original["transcription"]["prompt"] = prompt.clone();
            original["post_process"]["prompt"] = prompt.clone();
            // Cover both the branch's full/short files and older component files.
            original["dictation"]["prompt_components"] = if prompt.is_null() {
                let mut components = bundled["dictation"]["prompt_components"].clone();
                components["task"] = serde_json::json!("旧组件");
                components
            } else {
                serde_json::Value::Null
            };
            let old_short = if prompt.as_str() == Some("") {
                ""
            } else {
                "旧短版"
            };
            original["dictation"]["prompt"] = prompt;
            original["dictation"]["short_prompt"] = serde_json::json!(old_short);
            original["audio"]["mic_gain"] = serde_json::json!(1.75);
            original["input"]["hold_hotkey"] = serde_json::json!("F7");
            original["inference"]["api"]["transcription"]["api_key"] =
                serde_json::json!("disk-token");
            original["dictation"]["memory"] = serde_json::json!({"enabled":false,"long_term":[{"context":"开发","kind":"proper_noun","text":"ViberWhisper"}]});
            let bytes = serde_json::to_vec(&original).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            let loaded = store.load().unwrap().unwrap();
            assert_eq!(
                loaded.dictation.legacy_short_prompt.as_deref(),
                Some(old_short)
            );
            let mut serializable = original.clone();
            serializable["dictation"]
                .as_object_mut()
                .unwrap()
                .remove("short_prompt");
            assert_eq!(serde_json::to_value(&loaded).unwrap(), serializable);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            let prepared = store.prepare().unwrap().unwrap();
            let mut expected = original;
            for section in ["transcription", "post_process", "dictation"] {
                expected[section]["prompt"] = bundled[section]["prompt"].clone();
            }
            expected["dictation"]
                .as_object_mut()
                .unwrap()
                .remove("short_prompt");
            expected["dictation"]["prompt_components"] =
                bundled["dictation"]["prompt_components"].clone();
            let updated: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(updated, expected);
            assert_eq!(store.load().unwrap(), Some(prepared.clone()));
            let complete = std::fs::read(&path).unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            assert_eq!(store.prepare().unwrap(), Some(prepared));
            assert_eq!(std::fs::read(&path).unwrap(), complete);
            assert_eq!(
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                modified
            );
        }
    }

    #[test]
    fn startup_materializes_missing_dictation_but_load_stays_read_only() {
        // Upgrading an existing v3 file must expose defaults without enabling Realtime.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        let mut original = serde_json::to_value(ConfigDocument::default()).unwrap();
        original.as_object_mut().unwrap().remove("dictation");
        let bytes = serde_json::to_vec(&original).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.dictation.prompt.is_none());
        assert!(loaded.dictation.prompt_components.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let prepared = store.prepare().unwrap().unwrap();
        let updated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(updated["dictation"]["enabled"], false);
        assert!(updated["dictation"]["prompt"].is_null());
        assert!(updated["dictation"]["prompt_components"].is_object());
        assert!(updated["dictation"].get("short_prompt").is_none());
        let mut expected = original;
        expected["dictation"] = serde_json::to_value(&prepared.dictation).unwrap();
        expected["post_process"]["prompt"] =
            serde_json::to_value(&prepared.post_process.prompt).unwrap();
        assert_eq!(updated, expected);
        let complete = std::fs::read(&path).unwrap();
        store.prepare().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), complete);
    }

    #[test]
    fn startup_backfill_preserves_explicit_dictation_and_unrelated_settings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        let mut original: serde_json::Value =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        let mut components = original["dictation"]["prompt_components"].clone();
        components["task"] = serde_json::json!("旧任务");
        original["dictation"] = serde_json::json!({
            "enabled": true,
            "url": "ws://example.test:8080/v1/realtime",
            "model": "custom-model",
            "api_key_env": "",
            "prompt": null,
            "prompt_components": components,
            "memory": {"enabled": false}
        });
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        store.prepare().unwrap();
        let updated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        original["dictation"]["memory"]["long_term"] = serde_json::json!([]);
        let bundled: serde_json::Value =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        original["dictation"]["prompt"] = bundled["dictation"]["prompt"].clone();
        original["dictation"]["prompt_components"] =
            bundled["dictation"]["prompt_components"].clone();
        assert_eq!(updated, original);

        // Empty instructions in an old config are also replaced during preparation.
        original["dictation"]["prompt"] = serde_json::json!("");
        original["dictation"]["prompt_components"] = serde_json::Value::Null;
        original["dictation"]
            .as_object_mut()
            .unwrap()
            .remove("memory");
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        store.prepare().unwrap();
        let updated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut expected = original;
        for key in ["prompt", "prompt_components", "memory"] {
            expected["dictation"][key] = bundled["dictation"][key].clone();
        }
        assert_eq!(updated, expected);
    }

    #[test]
    fn preparation_rejects_missing_or_invalid_documents_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        assert_eq!(store.prepare().unwrap(), None);
        assert!(!path.exists());
        let valid: serde_json::Value =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        for (pointer, invalid) in [
            ("/schema_version", serde_json::json!(2)),
            ("/transcription/prompt", serde_json::json!(42)),
            ("/dictation/short_prompt", serde_json::json!([])),
            (
                "/dictation/prompt_components",
                serde_json::json!({"task":"incomplete"}),
            ),
        ] {
            let mut value = valid.clone();
            value["dictation"]["short_prompt"] = serde_json::Value::Null;
            *value.pointer_mut(pointer).unwrap() = invalid;
            let bytes = serde_json::to_vec(&value).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(store.prepare().is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::write(&path, "invalid json").unwrap();
        assert!(store.prepare().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid json");
    }

    #[test]
    fn field_get_set_distinguishes_unknown_unset_and_read_only() {
        let mut document = ConfigDocument::new(MapSecrets(HashMap::new()));
        document.inference.api.post_process.model = None;
        assert_eq!(
            document
                .get_field("inference.api.post_process.model")
                .unwrap(),
            FieldValue::Unset
        );
        assert!(matches!(
            document.get_field("model"),
            Err(FieldError::UnknownKey(_))
        ));
        assert!(matches!(
            document.set_field("chunking.max_retries", "1"),
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
            document.get_field("audio.mic_gain").unwrap(),
            FieldValue::Value("2.5".to_string())
        );
        assert!(document.set_field("audio.mic_gain", "NaN").is_err());
        document
            .set_field("audio.input_device", "External USB Mic")
            .unwrap();
        assert_eq!(
            document.get_field("audio.input_device").unwrap(),
            FieldValue::Value("External USB Mic".to_string())
        );
        document.set_field("audio.input_device", "null").unwrap();
        assert_eq!(
            document.get_field("audio.input_device").unwrap(),
            FieldValue::Unset
        );
    }

    #[test]
    fn secret_status_reports_disk_environment_and_override_without_values() {
        let mut document = ConfigDocument::new(MapSecrets(HashMap::new()));
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key")
                .unwrap(),
            FieldValue::Secret(SecretStatus::Unset)
        );
        document.inference.api.transcription.api_key = Some("disk-token".to_string());
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key")
                .unwrap(),
            FieldValue::Secret(SecretStatus::Disk)
        );
        let environment = MapSecrets(HashMap::from([(
            "TRANSCRIPTION_API_KEY",
            "environment-token",
        )]));
        let mut document = ConfigDocument::new(environment);
        document.set_transcription_api_key(Some("disk-token".to_string()));
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key")
                .unwrap(),
            FieldValue::Secret(SecretStatus::EnvironmentOverridesDisk)
        );
        document.inference.api.transcription.api_key = None;
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key")
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

        let mut document = ConfigDocument::new(environment);
        assert_eq!(
            document
                .get_field("inference.api.transcription.api_key")
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

    #[test]
    fn field_selection_builds_caller_owned_settings_without_reading_unrelated_secrets() {
        struct NoSecrets;
        impl SecretSource for NoSecrets {
            fn get(&self, _name: &str) -> Option<String> {
                panic!("audio settings must not request API credentials");
            }
        }
        let mut document = ConfigDocument::new(NoSecrets);
        document.audio.input_device = Some("Desk microphone".to_string());
        document.audio.mic_gain = 1.5;
        // A caller requesting audio settings must not resolve unrelated API credentials.
        let selected = document.select(
            (fields::AudioInputDevice, fields::AudioMicGain),
            |settings| settings,
        );
        assert_eq!(selected, (Some("Desk microphone".to_string()), 1.5));
    }

    #[test]
    fn field_selection_resolves_only_the_requested_api_key() {
        struct TranscriptionSecret;
        impl SecretSource for TranscriptionSecret {
            fn get(&self, name: &str) -> Option<String> {
                assert_eq!(name, "TRANSCRIPTION_API_KEY");
                Some("environment-token".to_string())
            }
        }
        let mut document = ConfigDocument::new(TranscriptionSecret);
        document.set_transcription_api_key(Some("disk-token".to_string()));
        // STT-only workflows must not consult post-processing secrets while resolving auth.
        let auth = document.select(fields::ApiTranscriptionKey, |key| key.auth);
        let ApiAuth::Bearer(secret) = &auth else {
            panic!("the configured API key must resolve to bearer auth");
        };
        assert_eq!(secret.expose(), "environment-token");
        let debug = format!("{auth:?}");
        assert!(!debug.contains("environment-token"));
        assert!(!debug.contains("disk-token"));
    }
}
