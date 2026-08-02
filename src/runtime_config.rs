//! Application-level configuration assembly.
//!
//! This module selects the active inference profile, constructs consumer configs,
//! and aggregates construction issues into the few top-level workflow configs.
//! Persistence and business rules stay elsewhere.

use std::path::Path;

use crate::audio::AudioConfig;
use crate::core::config::{
    ApiAuth, ConfigDocument, InferenceProfile, SecretSource, SecretValue, ValidationIssue,
    ValidationReport,
};
use crate::core::orchestrator::OrchestratorConfig;
use crate::input::hotkey::HotkeyConfig;
use crate::local::{LocalPaths, LocalServiceConfig, MODEL_NAME as LOCAL_MODEL_NAME};
use crate::postprocess::PostProcessConfig;
use crate::transcriber::TranscriberConfig;

/// Chooses whether listener assembly follows the configured profile or forces Local.
pub enum ProfileSelection {
    Configured,
    Local,
}

/// Runtime dependencies shared by the listener workflow.
pub struct ListenerConfig {
    pub hotkeys: HotkeyConfig,
    pub audio: AudioConfig,
    pub orchestrator: OrchestratorConfig,
    pub backend: BackendConfig,
}

/// Runtime backend dependencies after the configured profile has been selected.
#[derive(Debug)]
pub struct BackendConfig {
    pub(crate) transcriber: TranscriberConfig,
    pub(crate) post_process: PostProcessConfig,
    pub(crate) local_service: Option<LocalServiceConfig>,
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
    selection: ProfileSelection,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<ListenerConfig, ValidationReport> {
    let mut issues = Vec::new();
    let hotkeys = collect_issues(HotkeyConfig::validate(&document.input), &mut issues);
    let audio = AudioConfig::from_sections(&document.audio, &document.chunking);
    let orchestrator = collect_issues(
        OrchestratorConfig::validate(&document.session, document.transcription.language.clone()),
        &mut issues,
    );
    let backend = collect_issues(
        resolve_backend(document, secrets, selection, config_dir, home_dir),
        &mut issues,
    );

    match (hotkeys, orchestrator, backend) {
        (Some(hotkeys), Some(orchestrator), Some(backend)) if issues.is_empty() => {
            Ok(ListenerConfig {
                hotkeys,
                audio,
                orchestrator,
                backend,
            })
        }
        _ => Err(report(issues)),
    }
}

/// Resolve the selected transcription backend for one offline conversion.
pub fn resolve_convert(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<ConvertConfig, ValidationReport> {
    let backend = resolve_backend(
        document,
        secrets,
        ProfileSelection::Configured,
        config_dir,
        home_dir,
    )
    .map_err(report)?;
    Ok(ConvertConfig {
        backend,
        language: document.transcription.language.clone(),
        max_chunk_duration_secs: document.chunking.max_duration_secs,
        max_chunk_size_bytes: document.chunking.max_size_bytes,
    })
}

/// Resolve Local filesystem paths without requiring the service to be valid.
pub fn resolve_local_paths(
    document: &ConfigDocument,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<LocalPaths, ValidationReport> {
    LocalPaths::resolve(&document.inference.local, config_dir, home_dir).map_err(report)
}

/// Resolve Local paths and service settings together.
pub fn resolve_local_service(
    document: &ConfigDocument,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<LocalServiceConfig, ValidationReport> {
    let paths = resolve_local_paths(document, config_dir, home_dir)?;
    LocalServiceConfig::validate(&document.inference.local, paths).map_err(report)
}

/// Check the configuration used by the active listener profile.
pub fn check(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<(), ValidationReport> {
    resolve_listener(
        document,
        secrets,
        ProfileSelection::Configured,
        config_dir,
        home_dir,
    )
    .map(|_| ())
}

fn resolve_backend(
    document: &ConfigDocument,
    secrets: &dyn SecretSource,
    selection: ProfileSelection,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<BackendConfig, Vec<ValidationIssue>> {
    let selected = match selection {
        ProfileSelection::Configured => document.inference.active,
        ProfileSelection::Local => InferenceProfile::Local,
    };
    match selected {
        InferenceProfile::Api => resolve_api_backend(document, secrets),
        InferenceProfile::Local => resolve_local_backend(document, config_dir, home_dir),
    }
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
            &document.chunking,
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
            local_service: None,
        }),
        _ => Err(issues),
    }
}

fn resolve_local_backend(
    document: &ConfigDocument,
    config_dir: &Path,
    home_dir: &Path,
) -> Result<BackendConfig, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let paths = collect_issues(
        LocalPaths::resolve(&document.inference.local, config_dir, home_dir),
        &mut issues,
    );
    let service = paths.and_then(|paths| {
        collect_issues(
            LocalServiceConfig::validate(&document.inference.local, paths),
            &mut issues,
        )
    });
    let base_url = format!("http://127.0.0.1:{}", document.inference.local.server_port);
    let transcription_url = format!("{base_url}/v1/audio/transcriptions");
    let transcriber = collect_issues(
        TranscriberConfig::validate(
            &transcription_url,
            ApiAuth::None,
            LOCAL_MODEL_NAME,
            &document.transcription,
            &document.chunking,
        ),
        &mut issues,
    );
    let post_process_url = format!("{base_url}/v1/chat/completions");
    let post_process = collect_issues(
        PostProcessConfig::validate(
            Some(&post_process_url),
            ApiAuth::None,
            Some(LOCAL_MODEL_NAME),
            &document.post_process,
        ),
        &mut issues,
    );
    match (service, transcriber, post_process) {
        (Some(service), Some(transcriber), Some(post_process)) if issues.is_empty() => {
            Ok(BackendConfig {
                transcriber,
                post_process,
                local_service: Some(service),
            })
        }
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
    use std::path::Path;

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

        let config = resolve_listener(
            &document,
            &secrets,
            ProfileSelection::Configured,
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();

        assert!(config.backend.local_service.is_none());
        assert_eq!(config.hotkeys.hold_label.as_deref(), Some("F8"));
    }

    #[test]
    fn local_override_does_not_mutate_persisted_profile() {
        let document = ConfigDocument::default();
        let secrets = MapSecrets(HashMap::new());

        let config = resolve_listener(
            &document,
            &secrets,
            ProfileSelection::Local,
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();

        assert!(config.backend.local_service.is_some());
        assert_eq!(
            document
                .get_field("inference.active", &secrets)
                .unwrap()
                .to_string(),
            "api"
        );
    }

    #[test]
    fn api_backend_allows_unauthenticated_compatible_endpoints() {
        let mut document = ConfigDocument::default();
        document.post_process.enabled = true;
        document.inference.api.post_process.api_url =
            Some("http://127.0.0.1:8080/v1/chat/completions".to_string());
        document.inference.api.post_process.model = Some("local-model".to_string());
        let secrets = MapSecrets(HashMap::new());

        let config = resolve_convert(
            &document,
            &secrets,
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();

        assert!(config.backend.local_service.is_none());
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

        let backend = resolve_convert(
            &document,
            &secrets,
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();
        let debug = format!("{backend:?}");
        assert!(!debug.contains("environment-token"));
        assert!(!debug.contains("disk-token"));
    }

    #[test]
    fn check_ignores_the_inactive_profile() {
        let mut document = ConfigDocument::default();
        document.inference.active = InferenceProfile::Local;
        document.inference.api.transcription.api_url = "not a URL".to_string();
        document.inference.api.transcription.model.clear();

        check(
            &document,
            &MapSecrets(HashMap::new()),
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();

        document.inference.active = InferenceProfile::Api;
        assert!(
            check(
                &document,
                &MapSecrets(HashMap::new()),
                Path::new("/config/viberwhisper"),
                Path::new("/home/test"),
            )
            .is_err()
        );
    }
}
