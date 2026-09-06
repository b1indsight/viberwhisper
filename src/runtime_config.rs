//! Application-level configuration assembly.
//!
//! This module constructs API consumer configs and aggregates construction issues into the few
//! top-level workflow configs.
//! Persistence and business rules stay elsewhere.

use crate::audio::{AudioConfig, MAX_CHUNK_DURATION_SECS, MAX_CHUNK_SIZE_BYTES};
use crate::core::config::{
    ApiAuth, ConfigDocument, SecretSource, SecretValue, ValidationIssue, ValidationReport,
};
use crate::core::orchestrator::OrchestratorConfig;
use crate::input::hotkey::HotkeyConfig;
use crate::postprocess::PostProcessConfig;
use crate::transcriber::TranscriberConfig;

/// Runtime dependencies shared by the listener workflow.
pub struct ListenerConfig {
    pub hotkeys: HotkeyConfig,
    pub audio: AudioConfig,
    pub orchestrator: OrchestratorConfig,
    pub backend: BackendConfig,
}

/// Runtime dependencies for transcription and optional post-processing.
#[derive(Debug)]
pub struct BackendConfig {
    pub(crate) transcriber: TranscriberConfig,
    pub(crate) post_process: PostProcessConfig,
}

/// Runtime dependencies and chunking policy for one offline WAV conversion.
#[derive(Debug)]
pub struct ConvertConfig {
    pub backend: BackendConfig,
    pub language: Option<String>,
    pub max_chunk_duration_secs: u32,
    pub max_chunk_size_bytes: u64,
}

/// Resolve every dependency needed by the long-running listener.
pub fn resolve_listener(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
) -> Result<ListenerConfig, ValidationReport> {
    let mut issues = Vec::new();
    let hotkeys = collect_issues(
        crate::platform::validate_hotkeys(&document.input),
        &mut issues,
    );
    let audio = AudioConfig::from_section(&document.audio);
    let orchestrator = OrchestratorConfig::new(document.transcription.language.clone());
    let backend = collect_issues(resolve_api_backend(document, secrets), &mut issues);

    match (hotkeys, backend) {
        (Some(hotkeys), Some(backend)) if issues.is_empty() => Ok(ListenerConfig {
            hotkeys,
            audio,
            orchestrator,
            backend,
        }),
        _ => Err(report(issues)),
    }
}

/// Resolve the transcription backend for one offline conversion.
pub fn resolve_convert(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
) -> Result<ConvertConfig, ValidationReport> {
    let backend = resolve_api_backend(document, secrets).map_err(report)?;
    Ok(ConvertConfig {
        backend,
        language: document.transcription.language.clone(),
        max_chunk_duration_secs: MAX_CHUNK_DURATION_SECS,
        max_chunk_size_bytes: MAX_CHUNK_SIZE_BYTES,
    })
}

/// Check the configuration used by the listener.
pub fn check(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
) -> Result<(), ValidationReport> {
    resolve_listener(document, secrets).map(|_| ())
}

fn resolve_api_backend(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
) -> Result<BackendConfig, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let transcription_secret = effective_secret(
        secrets.get("TRANSCRIPTION_API_KEY"),
        document.inference.api.transcription.api_key.as_deref(),
    );
    let transcriber = collect_issues(
        TranscriberConfig::validate(
            &document.inference.api.transcription.api_url,
            transcription_secret.map_or(ApiAuth::None, ApiAuth::Bearer),
            &document.inference.api.transcription.model,
            &document.transcription,
        ),
        &mut issues,
    );

    let post_secret = effective_secret(
        secrets.get("POST_PROCESS_API_KEY"),
        document.inference.api.post_process.api_key.as_deref(),
    );
    let post_process = collect_issues(
        PostProcessConfig::validate(
            document.inference.api.post_process.api_url.as_deref(),
            post_secret.map_or(ApiAuth::None, ApiAuth::Bearer),
            document.inference.api.post_process.model.as_deref(),
            &document.post_process,
        ),
        &mut issues,
    );

    match (transcriber, post_process) {
        (Some(transcriber), Some(post_process)) if issues.is_empty() => Ok(BackendConfig {
            transcriber,
            post_process,
        }),
        _ => Err(issues),
    }
}

fn effective_secret(environment: Option<String>, disk: Option<&str>) -> Option<SecretValue> {
    environment
        .filter(|value| !value.is_empty())
        .or_else(|| disk.filter(|value| !value.is_empty()).map(str::to_string))
        .map(SecretValue::new)
}

fn collect_issues<T>(
    result: Result<T, Vec<ValidationIssue>>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(errors) => {
            issues.extend(errors);
            None
        }
    }
}

fn report(mut issues: Vec<ValidationIssue>) -> ValidationReport {
    issues.sort_by(|left, right| {
        (left.key.as_str(), left.code).cmp(&(right.key.as_str(), right.code))
    });
    issues.dedup_by(|left, right| left.key == right.key && left.code == right.code);
    ValidationReport { issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{ConfigDocument, SecretSource};
    use std::collections::HashMap;

    struct MapSecrets(HashMap<&'static str, &'static str>);

    impl SecretSource for MapSecrets {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|value| (*value).to_string())
        }
    }

    #[test]
    fn resolves_api_listener_from_module_owned_configs() {
        let document = ConfigDocument::default();
        let secrets = MapSecrets(HashMap::from([("TRANSCRIPTION_API_KEY", "token")]));

        let config = resolve_listener(&document, &secrets).unwrap();

        assert_eq!(config.hotkeys.hold_label.as_deref(), Some("F8"));
    }

    #[test]
    fn api_backend_allows_unauthenticated_localhost_endpoints() {
        let mut document = ConfigDocument::default();
        document.post_process.enabled = true;
        document.inference.api.post_process.api_url =
            Some("http://127.0.0.1:8080/v1/chat/completions".to_string());
        document.inference.api.post_process.model = Some("local-model".to_string());
        let secrets = MapSecrets(HashMap::new());

        let config = resolve_convert(&document, &secrets).unwrap();

        assert_eq!(config.max_chunk_duration_secs, 30);
        assert_eq!(config.max_chunk_size_bytes, 23 * 1024 * 1024);
    }

    #[test]
    fn environment_secret_overrides_disk_without_debug_leakage() {
        let secret =
            effective_secret(Some("environment-token".to_string()), Some("disk-token")).unwrap();
        assert_eq!(secret.expose(), "environment-token");

        let mut document = ConfigDocument::default();
        document.inference.api.transcription.api_key = Some("disk-token".to_string());
        let secrets = MapSecrets(HashMap::from([(
            "TRANSCRIPTION_API_KEY",
            "environment-token",
        )]));

        let backend = resolve_convert(&document, &secrets).unwrap();
        let debug = format!("{backend:?}");
        assert!(!debug.contains("environment-token"));
        assert!(!debug.contains("disk-token"));
    }

    #[test]
    fn check_validates_the_api_configuration() {
        let mut document = ConfigDocument::default();
        document.inference.api.transcription.api_url = "not a URL".to_string();
        document.inference.api.transcription.model.clear();

        assert!(check(&document, &MapSecrets(HashMap::new())).is_err());
    }
}
