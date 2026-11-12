//! Chooses a recording-session implementation without changing the UI state machine.
use super::{
    config::{ConfigDocument, SecretValue, fields},
    orchestrator::{OrchestratorConfig, SessionError, SessionOrchestrator},
};
use crate::{
    dictation::{Confirmation, Transcript, config::DictationConfig},
    postprocess::PostProcessor,
    transcriber::{ApiTranscriber, TranscriberConfig, TranscriberMetadata},
};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

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
        let path = super::config::config_dir().map(|p| p.join("dictation-memory.json"));
        self.build_with_memory_path(path)
    }

    /// File evaluation supplies isolated storage; desktop keeps its discovered path.
    pub fn build_with_memory_path(self, path: Option<PathBuf>) -> Result<SessionOrchestrator> {
        match self {
            Self::Http {
                transcriber,
                orchestrator,
            } => {
                let transcriber = Arc::new(ApiTranscriber::new(transcriber)?);
                Ok(SessionOrchestrator::new(transcriber, orchestrator))
            }
            Self::Realtime {
                settings,
                key,
                language,
            } => SessionOrchestrator::new_realtime(settings, key, path, language),
        }
    }
}

/// Editable final text and the capability to learn only after successful acceptance.
#[derive(Debug)]
pub(crate) struct FinalizedTranscription {
    pub text: String,
    pub warning: Option<String>,
    pub confirmation: Option<Confirmation>,
}

impl From<(String, Option<String>)> for FinalizedTranscription {
    fn from((text, warning): (String, Option<String>)) -> Self {
        Self {
            text,
            warning,
            confirmation: None,
        }
    }
}

/// Shared desktop finalization, retaining warnings when no text can be delivered.
pub(crate) fn finish_transcription(
    result: Result<Transcript, SessionError>,
    post_processor: &PostProcessor,
) -> Option<FinalizedTranscription> {
    let (result, mut confirmation) = match result {
        Ok(transcript) => (Ok(transcript.text), transcript.confirmation),
        Err(error) => (Err(error), None),
    };
    let prepared = match result {
        Ok(stt_text) => {
            if stt_text.is_empty() {
                info!("Transcription returned empty text");
                return None;
            }
            let text = match post_processor.process_for_delivery(&stt_text) {
                Ok(processed) if !processed.text.is_empty() => {
                    confirmation = processed.confirmation.or(confirmation);
                    processed.text
                }
                Ok(_) => {
                    warn!("Post-processing returned empty text, using original STT text");
                    stt_text
                }
                Err(error) => {
                    warn!(error = %error, "Post-processing failed, using original STT text");
                    stt_text
                }
            };
            Some((text, None))
        }
        Err(SessionError::NoChunks) => {
            warn!("No audio chunks to transcribe");
            None
        }
        Err(SessionError::Routing(error)) => {
            error!(error = %error, "Session routing failed while finalizing");
            None
        }
        Err(SessionError::InputFailure {
            error,
            partial_text,
        }) => {
            error!(%error, "Recording input is incomplete");
            let warning = if partial_text.trim().is_empty() {
                "录音不完整，没有可输入的文字。\n按 Esc 关闭后重试。"
            } else {
                "录音不完整，请核对结果后输入。"
            };
            Some((partial_text, Some(warning.into())))
        }
        Err(SessionError::PartialFailure {
            errors,
            partial_text,
        }) => {
            error!(
                failed_chunks = errors.len(),
                "Partial transcription failure"
            );
            let warning = if partial_text.trim().is_empty() {
                "识别失败，没有可输入的文字。\n按 Esc 关闭后重试。"
            } else {
                "部分识别失败，请核对结果后输入。"
            };
            Some((partial_text, Some(warning.into())))
        }
        Err(SessionError::ConvergenceTimeout {
            pending_count,
            partial_text,
        }) => {
            warn!(pending_count, "Convergence timeout");
            let warning = if partial_text.trim().is_empty() {
                "识别等待超时，没有可输入的文字。\n按 Esc 关闭后重试。"
            } else {
                "识别等待超时，结果可能不完整，请核对后输入。"
            };
            Some((partial_text, Some(warning.into())))
        }
    };
    prepared
        .filter(|(text, warning)| !text.trim().is_empty() || warning.is_some())
        .map(|(text, warning)| FinalizedTranscription {
            text: if text.trim().is_empty() {
                String::new()
            } else {
                text
            },
            warning,
            confirmation,
        })
}
