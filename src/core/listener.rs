//! Settings shared by desktop recording, setup verification, and raw STT capture.

use anyhow::Result;

use crate::audio::AudioConfig;
use crate::core::config::{ConfigDocument, InputSection, fields};
use crate::core::orchestrator::OrchestratorConfig;
use crate::input::hotkey::HotkeyConfig;
use crate::postprocess::PostProcessConfig;
use crate::transcriber::TranscriberConfig;

/// Settings shared by live delivery and raw STT capture.
#[derive(Debug)]
pub(crate) struct RecordingConfig {
    pub(crate) hotkeys: HotkeyConfig,
    pub(crate) audio: AudioConfig,
    pub(crate) orchestrator: OrchestratorConfig,
    pub(crate) transcriber: TranscriberConfig,
}

impl RecordingConfig {
    pub(crate) fn from_config(document: &ConfigDocument) -> Result<Self> {
        let hotkeys = document.select(
            (fields::InputHoldHotkey, fields::InputToggleHotkey),
            |(hold_hotkey, toggle_hotkey)| {
                crate::platform::hotkey_config(&InputSection {
                    hold_hotkey,
                    toggle_hotkey,
                })
            },
        )?;
        Ok(Self {
            hotkeys,
            audio: AudioConfig::from_config(document),
            orchestrator: document.select(fields::TranscriptionLanguage, OrchestratorConfig::new),
            transcriber: TranscriberConfig::from_config(document)?,
        })
    }
}

/// Live text delivery adds optional cleanup to the recording settings.
#[derive(Debug)]
pub(crate) struct ListenerConfig {
    pub(crate) recording: RecordingConfig,
    pub(crate) post_process: PostProcessConfig,
}

impl ListenerConfig {
    pub(crate) fn from_config(document: &ConfigDocument) -> Result<Self> {
        Ok(Self {
            recording: RecordingConfig::from_config(document)?,
            post_process: PostProcessConfig::from_config(document)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ListenerConfig, RecordingConfig};
    use crate::core::config::{ConfigDocument, SecretSource};

    struct EmptySecrets;

    impl SecretSource for EmptySecrets {
        fn get(&self, _name: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn raw_capture_settings_ignore_invalid_cleanup_configuration() {
        let mut document = ConfigDocument::new(EmptySecrets);
        document.post_process.enabled = true;
        document.inference.api.post_process.api_url = Some("not a URL".to_string());
        // Dataset capture archives raw STT, so an incomplete cleanup setup must not block it.
        let config = RecordingConfig::from_config(&document).unwrap();
        assert_eq!(
            config.hotkeys.hold.map(|binding| binding.canonical),
            Some("F8")
        );
        assert!(ListenerConfig::from_config(&document).is_err());
    }
}
