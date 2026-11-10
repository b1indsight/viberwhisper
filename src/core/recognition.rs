//! Chooses a recording-session implementation without changing the UI state machine.
use super::{
    config::{ConfigDocument, SecretValue, fields},
    orchestrator::{OrchestratorConfig, SessionOrchestrator},
};
use crate::{
    audio::segment::Segmentation,
    dictation::config::DictationConfig,
    session::SessionId,
    transcriber::{ApiTranscriber, TranscriberConfig, TranscriberMetadata},
};
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum RecognitionConfig {
    Http {
        transcriber: TranscriberConfig,
        orchestrator: OrchestratorConfig,
        segmentation: Segmentation,
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
                segmentation: if raw_capture {
                    Segmentation::Fixed
                } else {
                    Segmentation::PauseAware
                },
            })
        }
    }

    pub fn is_realtime(&self) -> bool {
        matches!(self, Self::Realtime { .. })
    }
    pub fn uses_streaming_frames(&self) -> bool {
        self.is_realtime()
            || matches!(
                self,
                Self::Http {
                    segmentation: Segmentation::PauseAware,
                    ..
                }
            )
    }
    pub fn metadata(&self) -> Option<TranscriberMetadata> {
        match self {
            Self::Http { transcriber, .. } => Some(transcriber.metadata()),
            _ => None,
        }
    }
    pub fn build(
        self,
        notify: impl Fn(SessionId) + Send + Sync + 'static,
    ) -> Result<SessionOrchestrator> {
        match self {
            Self::Http {
                transcriber,
                orchestrator,
                segmentation,
            } => {
                let transcriber = Arc::new(ApiTranscriber::new(transcriber)?);
                Ok(match segmentation {
                    Segmentation::Fixed => SessionOrchestrator::new(transcriber, orchestrator),
                    Segmentation::PauseAware => {
                        SessionOrchestrator::new_live_http(transcriber, orchestrator, notify)
                    }
                })
            }
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
