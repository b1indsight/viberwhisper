use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{ConfigDocument, ConfigError};

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn discover() -> Result<Self, ConfigError> {
        let directory =
            crate::platform::config_dir().ok_or(ConfigError::ConfigDirectoryUnavailable)?;
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

    pub fn load(&self) -> Result<ConfigDocument, ConfigError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ConfigDocument::default());
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        serde_json::from_str(&contents).map_err(|source| ConfigError::InvalidDocument {
            path: self.path.clone(),
            source,
        })
    }

    pub fn save(&self, document: &ConfigDocument) -> Result<(), ConfigError> {
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
