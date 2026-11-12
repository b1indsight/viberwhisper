//! Production recognition assembly and finalization shared by live and file input.
use super::{
    config::{ConfigDocument, SecretValue, fields},
    orchestrator::{OrchestratorConfig, SessionError, SessionOrchestrator},
};
use crate::{
    dictation::{
        Clock, Progress, RealtimeMode, SegmentTrace, Snapshot, Transcript,
        config::{DictationConfig, MemoryConfig},
    },
    postprocess::{CleanupTrace, PostProcessConfig, PostProcessError, PostProcessor},
    session::SessionId,
    transcriber::{ApiTranscriber, TranscriberConfig, TranscriberMetadata},
};
use anyhow::{Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

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
        mode: RealtimeMode,
        asr_prompt_override: Option<String>,
    },
}

impl RecognitionConfig {
    pub fn from_config(document: &ConfigDocument, raw_capture: bool) -> Result<Self> {
        if document.dictation.enabled && !raw_capture {
            document.dictation.validate()?;
            let mode = if document.post_process.enabled {
                ensure!(
                    document
                        .dictation
                        .prompt
                        .as_ref()
                        .is_none_or(|p| p.is_empty())
                        && document
                            .dictation
                            .prompt_components
                            .as_ref()
                            .is_none_or(|p| p.compose().is_empty()),
                    "two-stage recognition conflicts with saved dictation.prompt / dictation.prompt_components; back up and clear these integrated overrides, and put cleanup rules in post_process.prompt"
                );
                RealtimeMode::Transcription
            } else {
                RealtimeMode::Integrated
            };
            Ok(Self::Realtime {
                settings: document.dictation.clone(),
                key: document.dictation_key(),
                language: document.transcription.language.clone(),
                mode,
                asr_prompt_override: None,
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
    /// Candidate instructions affect only the selected audio stage, never saved settings.
    pub fn with_prompt_override(self, prompt: Option<String>) -> Self {
        match self {
            Self::Http {
                transcriber,
                orchestrator,
            } => Self::Http {
                transcriber: match prompt {
                    Some(prompt) => transcriber.with_prompt((!prompt.is_empty()).then_some(prompt)),
                    None => transcriber,
                },
                orchestrator,
            },
            Self::Realtime {
                settings,
                key,
                language,
                mode,
                ..
            } => Self::Realtime {
                settings,
                key,
                language,
                mode,
                asr_prompt_override: prompt,
            },
        }
    }

    pub fn build(self, memory_path: Option<PathBuf>, clock: Clock) -> Result<SessionOrchestrator> {
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
                mode,
                asr_prompt_override,
            } => SessionOrchestrator::new_realtime(
                settings,
                key,
                memory_path,
                language,
                mode,
                clock,
                asr_prompt_override,
            ),
        }
    }
}

/// The same session and optional text-learning owner for desktop, lab and conversion.
pub(crate) struct RecognitionPipeline {
    pub orchestrator: Arc<SessionOrchestrator>,
    pub post_processor: Arc<PostProcessor>,
    pub streaming: bool,
    integrated: bool,
    language: Option<String>,
}

#[derive(Debug)]
pub(crate) enum CleanupStatus {
    Skipped,
    Disabled,
    Applied,
    EmptyFallback,
    FailedFallback(PostProcessError),
}

impl CleanupStatus {
    #[cfg(test)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Disabled => "disabled",
            Self::Applied => "applied",
            Self::EmptyFallback => "empty_fallback",
            Self::FailedFallback(_) => "failed_fallback",
        }
    }
}

#[derive(Debug)]
pub(crate) struct RecognitionOutcome {
    pub raw_text: String,
    pub transcript: Option<Transcript>,
    pub warning: Option<String>,
    pub recognition_error: Option<SessionError>,
    pub cleanup: CleanupStatus,
    pub playback_time: Duration,
    pub recognition_wait: Duration,
    pub cleanup_time: Duration,
    pub cleanup_trace: Option<CleanupTrace>,
    pub asr_traces: Vec<SegmentTrace>,
}

impl RecognitionPipeline {
    /// The host chooses storage and clock explicitly; this never discovers personal state.
    pub fn from_config(
        document: &ConfigDocument,
        memory_path: Option<PathBuf>,
        clock: Clock,
        asr_prompt_override: Option<String>,
    ) -> Result<Self> {
        let recognition = RecognitionConfig::from_config(document, false)?
            .with_prompt_override(asr_prompt_override);
        Self::from_resolved(
            recognition,
            PostProcessConfig::from_config(document)?,
            document.dictation.memory.clone(),
            document.transcription.language.clone(),
            memory_path,
            clock,
        )
    }

    pub fn from_resolved(
        recognition: RecognitionConfig,
        post_process: PostProcessConfig,
        memory: MemoryConfig,
        language: Option<String>,
        memory_path: Option<PathBuf>,
        clock: Clock,
    ) -> Result<Self> {
        let streaming = recognition.is_realtime();
        let integrated = matches!(
            recognition,
            RecognitionConfig::Realtime {
                mode: RealtimeMode::Integrated,
                ..
            }
        );
        let orchestrator = Arc::new(recognition.build(memory_path.clone(), clock.clone())?);
        let post_processor = Arc::new(PostProcessor::with_memory(
            post_process,
            memory,
            memory_path,
            language.clone(),
            clock,
        )?);
        Ok(Self {
            orchestrator,
            post_processor,
            streaming,
            integrated,
            language,
        })
    }

    pub fn finish(&self, session_id: SessionId, stopped_at: Instant) -> RecognitionOutcome {
        let result = self.orchestrator.finish_for_delivery(session_id);
        let recognition_wait = stopped_at.elapsed();
        let mut outcome = finalize(result, &self.post_processor);
        outcome.recognition_wait = recognition_wait;
        outcome.asr_traces = self.orchestrator.take_traces();
        if self.integrated {
            // Integrated delivery already applies confirmed local corrections.
            // Reports retain those final edits separately from actual model text.
            let texts = outcome
                .asr_traces
                .iter()
                .map(|trace| {
                    trace.output["text"]
                        .as_str()
                        .expect("validated integrated result has text")
                        .to_owned()
                })
                .collect::<Vec<_>>();
            outcome.raw_text = crate::text::merge_texts(&texts, self.language.clone());
        }
        outcome
    }

    /// Replace only microphone capture: PCM buffering, session routing and finalization remain production code.
    pub fn recognize_wav(
        &self,
        path: &Path,
        session_id: SessionId,
        cancelled: &CancellationToken,
    ) -> Result<RecognitionOutcome> {
        ensure!(!cancelled.is_cancelled(), "WAV recognition cancelled");
        // Queue faults cancel intake too, but only the host can cancel the parent.
        // Keep those failures on production's partial-result finalization path.
        let intake = cancelled.child_token();
        let mut progress = Progress::new(|| {});
        progress.cancelled = intake.clone();
        self.orchestrator
            .start_session_with_progress(session_id, Some(progress))?;
        let started = Instant::now();
        let input = crate::audio::replay_wav(path, session_id, self.streaming, &intake, |chunk| {
            self.orchestrator.on_chunk_ready(session_id, chunk);
        });
        let stopped_at = Instant::now();
        if cancelled.is_cancelled() {
            let _ = self.orchestrator.abort_session(session_id);
            anyhow::bail!("WAV recognition cancelled");
        }
        if let Err(error) = input
            && self.orchestrator.recording_error(session_id).is_none()
        {
            self.orchestrator
                .fail_session(session_id, &error.to_string());
        }
        let mut outcome = self.finish(session_id, stopped_at);
        outcome.playback_time = stopped_at.duration_since(started);
        ensure!(!cancelled.is_cancelled(), "WAV recognition cancelled");
        Ok(outcome)
    }

    /// Resolve pending confirmation before calling this barrier.
    pub fn snapshot_with_time(&self) -> Result<(Snapshot, Option<u64>)> {
        if self.integrated {
            self.orchestrator
                .snapshot_with_time()
                .map(|(snapshot, at)| (snapshot, Some(at)))
        } else {
            self.post_processor.snapshot_with_time()
        }
    }

    pub fn shutdown(&self) {
        self.orchestrator.shutdown();
        self.post_processor.shutdown();
    }
}

/// Preserve production partial-result warnings and confirmation ownership at the pre-edit boundary.
pub(crate) fn finalize(
    result: std::result::Result<Transcript, SessionError>,
    post_processor: &PostProcessor,
) -> RecognitionOutcome {
    let mut outcome = RecognitionOutcome {
        raw_text: String::new(),
        transcript: None,
        warning: None,
        recognition_error: None,
        cleanup: CleanupStatus::Skipped,
        playback_time: Duration::ZERO,
        recognition_wait: Duration::ZERO,
        cleanup_time: Duration::ZERO,
        cleanup_trace: None,
        asr_traces: Vec::new(),
    };
    match result {
        Ok(mut transcript) => {
            outcome.raw_text = transcript.text.clone();
            if transcript.text.trim().is_empty() {
                return outcome;
            }
            let started = Instant::now();
            match post_processor.process_with_trace(&transcript.text) {
                Ok((processed, trace)) if !processed.text.trim().is_empty() => {
                    transcript.text = processed.text;
                    transcript.confirmation = processed.confirmation.or(transcript.confirmation);
                    outcome.cleanup = if matches!(post_processor, PostProcessor::Disabled) {
                        CleanupStatus::Disabled
                    } else {
                        CleanupStatus::Applied
                    };
                    outcome.cleanup_trace = trace;
                }
                Ok(_) | Err(PostProcessError::EmptyContent) => {
                    warn!("Post-processing returned empty text, using original STT text");
                    outcome.cleanup = CleanupStatus::EmptyFallback;
                }
                Err(error) => {
                    warn!(%error, "Post-processing failed, using original STT text");
                    outcome.cleanup = CleanupStatus::FailedFallback(error);
                }
            }
            if !matches!(post_processor, PostProcessor::Disabled) {
                outcome.cleanup_time = started.elapsed();
            }
            outcome.transcript = Some(transcript);
        }
        Err(error) => {
            let (text, warning) = match &error {
                SessionError::NoChunks => (None, None),
                SessionError::Routing(error) => {
                    error!(%error, "Session routing failed while finalizing");
                    (None, None)
                }
                SessionError::InputFailure { partial_text, .. } => (
                    Some(partial_text),
                    Some(if partial_text.trim().is_empty() {
                        "录音不完整，没有可输入的文字。\n按 Esc 关闭后重试。"
                    } else {
                        "录音不完整，请核对结果后输入。"
                    }),
                ),
                SessionError::PartialFailure { partial_text, .. } => (
                    Some(partial_text),
                    Some(if partial_text.trim().is_empty() {
                        "识别失败，没有可输入的文字。\n按 Esc 关闭后重试。"
                    } else {
                        "部分识别失败，请核对结果后输入。"
                    }),
                ),
                SessionError::ConvergenceTimeout { partial_text, .. } => (
                    Some(partial_text),
                    Some(if partial_text.trim().is_empty() {
                        "识别等待超时，没有可输入的文字。\n按 Esc 关闭后重试。"
                    } else {
                        "识别等待超时，结果可能不完整，请核对后输入。"
                    }),
                ),
            };
            if let Some(text) = text {
                outcome.raw_text = text.clone();
                outcome.transcript = Some(
                    if text.trim().is_empty() {
                        String::new()
                    } else {
                        text.clone()
                    }
                    .into(),
                );
            }
            outcome.warning = warning.map(str::to_owned);
            outcome.recognition_error = Some(error);
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dictation::system_clock, postprocess::test_support::stub, session::SessionId};
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn realtime_cleanup_selects_audio_only_and_rejects_saved_integrated_prompts() {
        let mut document = ConfigDocument::default();
        document.dictation.enabled = true;
        document.post_process.enabled = true;
        assert!(matches!(
            RecognitionConfig::from_config(&document, false).unwrap(),
            RecognitionConfig::Realtime {
                mode: RealtimeMode::Transcription,
                ..
            }
        ));
        document.dictation.prompt = Some("旧的整理示例".into());
        let error = RecognitionConfig::from_config(&document, false).unwrap_err();
        assert!(error.to_string().contains("dictation.prompt"));
        document.post_process.enabled = false;
        assert!(matches!(
            RecognitionConfig::from_config(&document, false).unwrap(),
            RecognitionConfig::Realtime {
                mode: RealtimeMode::Integrated,
                ..
            }
        ));
    }

    #[test]
    fn shared_finalization_keeps_cleaned_text_and_reports_raw_fallback() {
        // The text stage deliberately changes the raw result. A lab that scores
        // raw STT or bypasses production fallback fails this same entry point.
        let (endpoint, requests) = stub(vec![
            json!({"text":"整理后的文字。","context":"项目","memory_candidates":[]}).to_string(),
            "invalid JSON".into(),
            json!({"text":"","context":"未知语境","memory_candidates":[]}).to_string(),
        ]);
        let mut document = ConfigDocument::default();
        document.post_process.enabled = true;
        document.post_process.preheat_enabled = false;
        document.inference.api.post_process.api_url = Some(endpoint.to_string());
        document.inference.api.post_process.model = Some("fixture".into());
        let pipeline =
            RecognitionPipeline::from_config(&document, None, system_clock(), None).unwrap();
        for status in ["applied", "failed_fallback", "empty_fallback"] {
            let outcome = finalize(Ok("原始文字".to_owned().into()), &pipeline.post_processor);
            assert_eq!(outcome.raw_text, "原始文字");
            assert_eq!(outcome.cleanup.as_str(), status);
            let expected = if status == "applied" {
                "整理后的文字。"
            } else {
                "原始文字"
            };
            assert_eq!(outcome.transcript.unwrap().text, expected);
            let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
            let input: serde_json::Value =
                serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap())
                    .unwrap();
            assert_eq!(input["text"], "原始文字");
        }
        let partial = finalize(
            Err(super::super::orchestrator::SessionError::InputFailure {
                error: "private-provider-error".into(),
                partial_text: "已经识别".into(),
            }),
            &pipeline.post_processor,
        );
        assert_eq!(partial.cleanup.as_str(), "skipped");
        assert_eq!(partial.transcript.unwrap().text, "已经识别");
        assert!(!partial.warning.unwrap().contains("private-provider-error"));
        pipeline.shutdown();
        // A routed finish cannot be presented as a successful empty recording.
        let missing = pipeline.finish(SessionId(99), Instant::now());
        assert!(missing.recognition_error.is_some());
        assert!(missing.transcript.is_none());
    }

    #[test]
    fn wav_worker_failure_keeps_incomplete_outcome_distinct_from_user_cancellation() {
        // A bounded-queue or recorder fault cancels worker intake. It must not
        // cancel the host's token and discard the normal incomplete-result path.
        use std::net::TcpListener;
        use tokio_tungstenite::tungstenite::{Message, accept};

        for user_cancelled in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut document = ConfigDocument::default();
            document.dictation.enabled = true;
            document.dictation.url = format!("ws://{}", listener.local_addr().unwrap());
            let pipeline = Arc::new(
                RecognitionPipeline::from_config(&document, None, system_clock(), None).unwrap(),
            );
            let cancelled = CancellationToken::new();
            let server_pipeline = Arc::clone(&pipeline);
            let server_cancelled = cancelled.clone();
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut socket = accept(stream).unwrap();
                socket
                    .send(Message::text(json!({"type":"session.created"}).to_string()))
                    .unwrap();
                let update: serde_json::Value =
                    serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(update["type"], "session.update");
                if user_cancelled {
                    server_cancelled.cancel();
                } else {
                    server_pipeline
                        .orchestrator
                        .fail_session(SessionId(1), "recorder overflow");
                }
            });
            let result = pipeline.recognize_wav(
                Path::new("tests/fixtures/audio/speech.wav"),
                SessionId(1),
                &cancelled,
            );
            server.join().unwrap();
            if user_cancelled {
                assert!(result.is_err());
                assert!(cancelled.is_cancelled());
            } else {
                let outcome = result.expect("worker failure must retain production error handling");
                assert!(!cancelled.is_cancelled());
                assert!(outcome.recognition_error.is_some());
                assert!(outcome.warning.is_some());
                assert_eq!(outcome.cleanup.as_str(), "skipped");
            }
            pipeline.shutdown();
        }
    }
}
