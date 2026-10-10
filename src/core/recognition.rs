//! Chooses a recording-session implementation without changing the UI state machine.
use super::{
    config::{ConfigDocument, SecretValue, fields},
    orchestrator::{OrchestratorConfig, SessionOrchestrator},
};
use crate::{
    dictation::config::DictationConfig,
    transcriber::{ApiTranscriber, TranscriberConfig, TranscriberMetadata},
};
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum RecognitionConfig {
    Http {
        transcriber: TranscriberConfig,
        orchestrator: OrchestratorConfig,
    },
    Realtime {
        settings: DictationConfig,
        key: Option<SecretValue>,
        language: Option<String>,
    },
}

impl RecognitionConfig {
    pub fn from_config(document: &ConfigDocument, raw_capture: bool) -> Result<Self> {
        if document.dictation.enabled && !raw_capture {
            document.dictation.validate()?;
            Ok(Self::Realtime {
                settings: document.dictation.clone(),
                key: document.dictation_key(),
                language: document.transcription.language.clone(),
            })
        } else {
            Ok(Self::Http {
                transcriber: TranscriberConfig::from_config(document)?,
                orchestrator: document
                    .select(fields::TranscriptionLanguage, OrchestratorConfig::new),
            })
        }
    }

    pub fn is_realtime(&self) -> bool {
        matches!(self, Self::Realtime { .. })
    }
    pub fn metadata(&self) -> Option<TranscriberMetadata> {
        match self {
            Self::Http { transcriber, .. } => Some(transcriber.metadata()),
            _ => None,
        }
    }
    pub fn build(self) -> Result<SessionOrchestrator> {
        match self {
            Self::Http {
                transcriber,
                orchestrator,
            } => Ok(SessionOrchestrator::new(
                Arc::new(ApiTranscriber::new(transcriber)?),
                orchestrator,
            )),
            Self::Realtime {
                settings,
                key,
                language,
            } => {
                let path = super::config::config_dir().map(|p| p.join("dictation-memory.json"));
                SessionOrchestrator::new_realtime(settings, key, path, language)
            }
        }
    }
}
