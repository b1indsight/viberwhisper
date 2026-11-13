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

    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the document with environment-backed secrets, or `None` for a missing file.
    pub fn load(&self) -> Result<Option<ConfigDocument>, ConfigError> {
        let Some(contents) = self.read()? else {
            return Ok(None);
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

    /// Updates only the input choice in the latest document; never replaces a missing config.
    pub(crate) fn set_input_device(&self, device: Option<&str>) -> Result<(), ConfigError> {
        let mut document = self.load()?.ok_or_else(|| ConfigError::Io {
            path: self.path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "configuration file is missing; run setup before saving a device",
            ),
        })?;
        document.audio.input_device = device.map(str::to_owned);
        self.save(&document)
    }

    /// Saves the slider's linear amplitude in the latest document, retaining unrelated edits.
    pub(crate) fn set_mic_gain(&self, gain: f32) -> Result<(), ConfigError> {
        let mut document = self.load()?.ok_or_else(|| ConfigError::Io {
            path: self.path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "configuration file is missing; run setup before saving microphone gain",
            ),
        })?;
        document.audio.mic_gain = gain;
        self.save(&document)
    }

    /// Refreshes bundled instructions on disk before recognition is assembled.
    /// Strict parsing precedes any write; readers and targeted tray edits remain read-only.
    pub(crate) fn prepare(&self) -> Result<Option<ConfigDocument>, ConfigError> {
        let Some(contents) = self.read()? else {
            return Ok(None);
        };
        serde_json::from_str::<ConfigDocument>(&contents).map_err(|source| {
            ConfigError::InvalidDocument {
                path: self.path.clone(),
                source,
            }
        })?;
        let mut value: serde_json::Value =
            serde_json::from_str(&contents).map_err(|source| ConfigError::InvalidDocument {
                path: self.path.clone(),
                source,
            })?;
        let original = value.clone();
        let defaults =
            serde_json::to_value(ConfigDocument::default()).map_err(ConfigError::Serialize)?;
        fill_missing(&mut value, &defaults);
        for section in ["transcription", "post_process", "dictation"] {
            value[section]["prompt"] = defaults[section]["prompt"].clone();
        }
        value["dictation"]["prompt_components"] =
            defaults["dictation"]["prompt_components"].clone();
        value["dictation"]
            .as_object_mut()
            .expect("validated dictation object")
            .remove("short_prompt");
        if value != original {
            self.write_atomic(&value)?;
        }
        self.load()
    }

    fn read(&self) -> Result<Option<String>, ConfigError> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ConfigError::Io {
                path: self.path.clone(),
                source,
            }),
        }
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

fn fill_missing(value: &mut serde_json::Value, defaults: &serde_json::Value) {
    let (Some(value), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) else {
        return;
    };
    for (key, default) in defaults {
        match value.entry(key.clone()) {
            serde_json::map::Entry::Vacant(entry) => {
                entry.insert(default.clone());
            }
            serde_json::map::Entry::Occupied(mut entry) => {
                fill_missing(entry.get_mut(), default);
            }
        }
    }
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
