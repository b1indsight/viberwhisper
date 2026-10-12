use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{ConfigDocument, ConfigError};

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn discover() -> Result<Self, ConfigError> {
        let directory = config_dir().ok_or(ConfigError::ConfigDirectoryUnavailable)?;
        Ok(Self {
            path: directory.join("config.json"),
        })
    }

    #[cfg(test)]
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the document with environment-backed secrets, or `None` for a missing file.
    pub fn load(&self) -> Result<Option<ConfigDocument>, ConfigError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        serde_json::from_str(&contents)
            .map(Some)
            .map_err(|source| ConfigError::InvalidDocument {
                path: self.path.clone(),
                source,
            })
    }

    pub fn save(&self, document: &ConfigDocument) -> Result<(), ConfigError> {
        self.write_atomic(document)
    }

    /// Persists absent Realtime fields after the desktop caller validates `document`.
    /// Explicit values and unrelated sections are retained; ordinary `load` stays read-only.
    pub(crate) fn materialize_dictation_defaults(
        &self,
        document: &ConfigDocument,
    ) -> Result<bool, ConfigError> {
        let contents = fs::read_to_string(&self.path).map_err(|source| ConfigError::Io {
            path: self.path.clone(),
            source,
        })?;
        let mut value: serde_json::Value =
            serde_json::from_str(&contents).map_err(|source| ConfigError::InvalidDocument {
                path: self.path.clone(),
                source,
            })?;
        let defaults = serde_json::to_value(&document.dictation).map_err(ConfigError::Serialize)?;
        let dictation = value
            .as_object_mut()
            .expect("validated configuration is an object")
            .entry("dictation")
            .or_insert_with(|| serde_json::json!({}));
        if !fill_missing(dictation, &defaults) {
            return Ok(false);
        }
        self.write_atomic(&value)?;
        Ok(true)
    }

    fn write_atomic(&self, document: &impl serde::Serialize) -> Result<(), ConfigError> {
        let parent = self.path.parent().ok_or_else(|| ConfigError::Io {
            path: self.path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "configuration path has no parent directory",
            ),
        })?;
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        serde_json::to_writer_pretty(&mut temporary, document).map_err(ConfigError::Serialize)?;
        temporary
            .write_all(b"\n")
            .map_err(|source| ConfigError::Io {
                path: self.path.clone(),
                source,
            })?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| ConfigError::Io {
                path: self.path.clone(),
                source,
            })?;
        temporary
            .persist(&self.path)
            .map_err(|error| ConfigError::Io {
                path: self.path.clone(),
                source: error.error,
            })?;
        Ok(())
    }
}

fn fill_missing(value: &mut serde_json::Value, defaults: &serde_json::Value) -> bool {
    let (Some(value), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) else {
        return false;
    };
    let mut changed = false;
    for (key, default) in defaults {
        match value.entry(key.clone()) {
            serde_json::map::Entry::Vacant(entry) => {
                entry.insert(default.clone());
                changed = true;
            }
            serde_json::map::Entry::Occupied(mut entry) => {
                changed |= fill_missing(entry.get_mut(), default);
            }
        }
    }
    changed
}

/// Canonical shared directory for configuration and local transcription history.
pub(crate) fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    const DIRECTORY: &str = "com.b1indsight.viberwhisper";
    #[cfg(target_os = "windows")]
    const DIRECTORY: &str = "ViberWhisper";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const DIRECTORY: &str = "viberwhisper";

    dirs::config_dir().map(|base| base.join(DIRECTORY))
}
