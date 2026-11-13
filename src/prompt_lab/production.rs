//! Ordinary evaluation replaces capture and delivery, leaving recognition to desktop owners.
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{SttSnapshot, dataset::sha256_bytes};
use crate::{
    core::{
        config::ConfigDocument,
        listener::ListenerConfig,
        orchestrator::{SessionError, SessionOrchestrator},
        recognition::{RecognitionConfig, finish_transcription},
    },
    dictation::Progress,
    postprocess::{PostProcessConfig, PostProcessor},
    session::SessionId,
};

/// Comparison identity excludes the candidate audio prompt and contains no credentials/state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub policy: String,
    pub config_sha256: String,
    pub memory_sha256: String,
}

impl Identity {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            matches!(
                self.policy.as_str(),
                "desktop-recognition-v1"
                    | "desktop-recognition-short-audio-v1"
                    | "desktop-recognition-reference-text-v1"
            ) && [&self.config_sha256, &self.memory_sha256]
                .into_iter()
                .all(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())),
            "unsupported production execution identity"
        );
        Ok(())
    }
}

/// A single run retains connections, real clocks and feedback ordering across all WAVs.
/// The temporary directory outlives both owners and is removed only after shutdown.
pub(crate) struct Runner {
    pub stt: SttSnapshot,
    pub identity: Identity,
    orchestrator: SessionOrchestrator,
    post_processor: PostProcessor,
    streaming: bool,
    next_session: AtomicU64,
    _directory: tempfile::TempDir,
}

impl Runner {
    pub fn new(
        document: &ConfigDocument,
        prompt: Option<String>,
        source_memory: Option<PathBuf>,
    ) -> Result<Self> {
        // This is the same configuration gate used by desktop, including Realtime's
        // existing cleanup suppression. Validate saved settings before the candidate.
        let config = ListenerConfig::from_config(document)?;
        let recognition = match config.recording.recognition {
            RecognitionConfig::Http {
                transcriber,
                orchestrator,
            } => RecognitionConfig::Http {
                transcriber: match prompt {
                    Some(prompt) => transcriber.with_prompt((!prompt.is_empty()).then_some(prompt)),
                    None => transcriber,
                },
                orchestrator,
            },
            RecognitionConfig::Realtime {
                mut settings,
                key,
                language,
            } => {
                if let Some(prompt) = prompt {
                    settings.prompt = Some(prompt);
                    settings.prompt_components = None;
                    settings.validate()?;
                }
                RecognitionConfig::Realtime {
                    settings,
                    key,
                    language,
                }
            }
        };
        let stt = match &recognition {
            RecognitionConfig::Http { transcriber, .. } => transcriber.metadata().into(),
            RecognitionConfig::Realtime {
                settings, language, ..
            } => SttSnapshot {
                endpoint: settings.url.clone(),
                model: settings.model.clone(),
                language: language.clone(),
                prompt: Some(settings.prompt()),
                short_prompt: settings.short_prompt(),
                temperature: 0.0,
                input_layout: Some(crate::dictation::INPUT_LAYOUT.into()),
            },
        };
        let directory = tempfile::tempdir()?;
        let memory = directory.path().join("dictation-memory.json");
        let owns_memory =
            recognition.is_realtime() || matches!(&config.post_process, PostProcessConfig::Llm(_));
        crate::dictation::copy_memory(
            source_memory.filter(|_| owns_memory),
            memory.clone(),
            config.memory.clone(),
            super::regression::unix_time_ms()? / 1000,
        )?;
        // Conservative compatibility: saved settings other than the audio prompt must
        // remain equal. Hash only a credential-free copy, never serialize it in reports.
        let mut settings = document.clone();
        settings.set_transcription_api_key(None);
        settings.set_post_process_api_key(None);
        settings.transcription.prompt = None;
        settings.dictation.prompt = None;
        settings.dictation.prompt_components = None;
        let identity = Identity {
            policy: if recognition.is_realtime() {
                "desktop-recognition-reference-text-v1"
            } else {
                "desktop-recognition-short-audio-v1"
            }
            .into(),
            config_sha256: sha256_bytes(&serde_json::to_vec(&settings)?),
            memory_sha256: sha256_bytes(&std::fs::read(&memory)?),
        };
        let streaming = recognition.is_realtime();
        // These are desktop's constructors, with only their storage path replaced.
        let post_processor = PostProcessor::new_desktop(
            config.post_process,
            config.memory,
            Some(memory.clone()),
            config.language,
        )?;
        let orchestrator = recognition.build_with_memory_path(Some(memory))?;
        Ok(Self {
            stt,
            identity,
            orchestrator,
            post_processor,
            streaming,
            next_session: AtomicU64::new(1),
            _directory: directory,
        })
    }

    /// Returns the actual pre-edit final and a safe incomplete-run reason, if any.
    /// References are deliberately absent from this API and from confirmation feedback.
    pub fn recognize(&self, path: &Path) -> (Option<String>, Option<String>) {
        let session = SessionId(self.next_session.fetch_add(1, Ordering::Relaxed));
        let progress = Progress::new(|| {});
        let cancelled = progress.cancelled.clone();
        if let Err(error) = self
            .orchestrator
            .start_session_with_progress(session, Some(progress))
        {
            tracing::warn!(%error, "Evaluation session could not start");
            return (None, Some("recognition session could not start".into()));
        }
        if let Err(error) =
            crate::audio::replay_wav(path, session, self.streaming, &cancelled, |chunk| {
                self.orchestrator.on_chunk_ready(session, chunk)
            })
            && self.orchestrator.recording_error(session).is_none()
        {
            self.orchestrator.fail_session(session, &error.to_string());
        }
        let result = self.orchestrator.finish_for_delivery(session);
        let mut error = result.as_ref().err().map(|error| {
            match error {
                SessionError::NoChunks => "no audio chunks to transcribe",
                SessionError::Routing(_) => "recognition routing failed",
                SessionError::InputFailure { .. } => "recording input is incomplete",
                SessionError::PartialFailure { .. } => "some audio chunks failed",
                SessionError::ConvergenceTimeout { .. } => "recognition convergence timed out",
            }
            .to_owned()
        });
        let finalized = finish_transcription(result, &self.post_processor);
        let text = finalized.map(|finalized| {
            // Simulate accepting main's displayed final unchanged, never the reference.
            // Existing serial owners apply feedback before their next recording/job.
            if error.is_none()
                && let Some(confirmation) = finalized.confirmation
                && confirmation.confirm(finalized.text.clone()).is_err()
            {
                error = Some("recognition feedback worker stopped".into());
            }
            finalized.text
        });
        if text.is_none() && error.is_none() {
            error = Some("recognition produced no final text".into());
        }
        (text, error)
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.orchestrator.shutdown();
        self.post_processor.shutdown();
    }
}

#[cfg(test)]
mod tests;
