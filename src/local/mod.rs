pub mod installer;
pub mod service;

pub use installer::{
    PythonRuntime, dependencies_installed, detect_python_runtime, download_model,
    install_requirements, model_weights_present, setup_venv, verify_install,
};
pub use service::LocalServiceManager;

use std::path::{Component, Path, PathBuf};

use crate::core::config::{ConfigKey, LocalSection, ValidationIssue};

pub(crate) const MODEL_NAME: &str = "gemma-4-E2B-it";

#[derive(Debug, PartialEq, Eq)]
pub struct LocalPaths {
    data_dir: PathBuf,
    pub(crate) venv_dir: PathBuf,
    pub(crate) model_dir: PathBuf,
}

impl LocalPaths {
    pub(crate) fn resolve(
        section: &LocalSection,
        config_dir: &Path,
        home_dir: &Path,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let configured = section.data_dir.as_deref().unwrap_or("~/.viberwhisper");
        if configured.trim().is_empty() {
            return Err(vec![ValidationIssue::new(
                ConfigKey::LocalDataDir,
                "local.data_dir_empty",
                "local data directory cannot be empty",
            )]);
        }

        let configured_path = Path::new(configured);
        let data_dir = if configured == "~" {
            home_dir.to_path_buf()
        } else if let Some(relative) = configured.strip_prefix("~/") {
            home_dir.join(relative)
        } else if configured_path.is_absolute() {
            configured_path.to_path_buf()
        } else {
            config_dir.join(configured_path)
        };
        let data_dir = normalize_lexically(&data_dir);

        Ok(Self {
            venv_dir: data_dir.join("venv"),
            model_dir: data_dir.join("model"),
            data_dir,
        })
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalQuantization {
    Int4,
    Int8,
    Bf16,
}

impl LocalQuantization {
    fn as_str(self) -> &'static str {
        match self {
            Self::Int4 => "int4",
            Self::Int8 => "int8",
            Self::Bf16 => "bf16",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LocalServiceConfig {
    pub(crate) paths: LocalPaths,
    port: u16,
    quantization: LocalQuantization,
}

impl LocalServiceConfig {
    pub(crate) fn validate(
        section: &LocalSection,
        paths: LocalPaths,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        if section.server_port == 0 {
            issues.push(ValidationIssue::new(
                ConfigKey::LocalServerPort,
                "local.port_zero",
                "local server port must be non-zero",
            ));
        }
        let quantization = match section.quantization.to_ascii_lowercase().as_str() {
            "int4" => Some(LocalQuantization::Int4),
            "int8" => Some(LocalQuantization::Int8),
            "bf16" => Some(LocalQuantization::Bf16),
            _ => {
                issues.push(ValidationIssue::new(
                    ConfigKey::LocalQuantization,
                    "local.quantization_invalid",
                    "quantization must be int4, int8, or bf16",
                ));
                None
            }
        };
        match quantization {
            Some(quantization) if issues.is_empty() => Ok(Self {
                paths,
                port: section.server_port,
                quantization,
            }),
            _ => Err(issues),
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use crate::core::config::LocalSection;
    use std::path::Path;

    #[test]
    fn resolves_local_paths_without_using_process_cwd() {
        let relative = LocalSection {
            data_dir: Some("models/local".to_string()),
            ..LocalSection::default()
        };
        let paths = LocalPaths::resolve(
            &relative,
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();
        assert_eq!(
            paths.data_dir,
            Path::new("/config/viberwhisper/models/local")
        );

        let home = LocalSection {
            data_dir: Some("~/.custom-viberwhisper".to_string()),
            ..LocalSection::default()
        };
        let paths = LocalPaths::resolve(
            &home,
            Path::new("/config/viberwhisper"),
            Path::new("/home/test"),
        )
        .unwrap();
        assert_eq!(paths.data_dir, Path::new("/home/test/.custom-viberwhisper"));
    }
}
