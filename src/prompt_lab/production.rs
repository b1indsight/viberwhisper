//! Production recognition execution and its independently verifiable report identity.
//!
//! References deliberately do not appear in this module's execution API. The host only supplies
//! audio, the saved stage configuration, an optional audio-stage candidate and explicit state.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{
    SttSnapshot,
    dataset::sha256_bytes,
    realtime::{Feedback, RealtimeCases},
};
use crate::{
    core::{
        config::ConfigDocument,
        recognition::{CleanupStatus, RecognitionConfig, RecognitionOutcome, RecognitionPipeline},
    },
    dictation::{
        Clock, DictationResult, MemorySeed, SegmentTrace, Snapshot, config::PromptComponents,
    },
    postprocess::{PostProcessConfig, PostProcessError},
    session::SessionId,
};

pub(super) const POLICY: &str = "production-recognition-v2";
const PACING: &str = "wav-mono-original-rate-no-gain-realtime-1x-v1";
const SEGMENTATION: &str = "production-pause-aware-v1";
const CLOCK: &str = "unix-seconds-start-plus-monotonic-elapsed-v1";
const FEEDBACK: &str = "explicit-delivered-only-v1";
const PRODUCTION_CLOCK: &str = "production-system-clock-v1";
const PRODUCTION_FEEDBACK: &str = "confirm-unedited-final-isolated-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    Http,
    Integrated,
    Transcription,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StageIdentity {
    pub endpoint: String,
    pub model: String,
    pub language: Option<String>,
    /// None means the protocol does not send a temperature field (Realtime).
    pub temperature: Option<f32>,
    pub prompt: Option<String>,
    pub prompt_sha256: String,
    pub input_layout: String,
}

impl StageIdentity {
    fn new(
        endpoint: &str,
        model: &str,
        language: Option<String>,
        temperature: Option<f32>,
        prompt: Option<String>,
        layout: &str,
    ) -> Result<Self> {
        let mut endpoint = reqwest::Url::parse(endpoint)?;
        let _ = endpoint.set_username("");
        let _ = endpoint.set_password(None);
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(Self {
            endpoint: endpoint.to_string(),
            model: model.into(),
            language,
            temperature,
            prompt_sha256: digest(&prompt)?,
            prompt,
            input_layout: layout.into(),
        })
    }

    fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.endpoint)?;
        ensure!(
            matches!(url.scheme(), "http" | "https" | "ws" | "wss")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "stage endpoint must be sanitized"
        );
        ensure!(
            !self.model.trim().is_empty()
                && self.temperature.is_none_or(f32::is_finite)
                && self.prompt_sha256 == digest(&self.prompt)?,
            "stage identity or prompt digest mismatch"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub policy: String,
    pub mode: Mode,
    pub audio: StageIdentity,
    pub cleanup: Option<StageIdentity>,
    pub cleanup_preheat: bool,
    pub candidate_override: bool,
    pub segmentation: String,
    pub pacing: String,
    pub memory_enabled: bool,
    pub memory_policy: String,
    pub seed_sha256: String,
    pub clock: String,
    pub feedback: String,
    pub cases_contract_sha256: Option<String>,
    pub sha256: String,
}

impl Identity {
    /// Resolve the same selected stages as production before applying a candidate. This prevents
    /// an override from concealing an incompatible saved integrated prompt in two-stage mode.
    pub fn from_config(
        document: &ConfigDocument,
        candidate: Option<&str>,
        cases: Option<&RealtimeCases>,
        seed: &MemorySeed,
    ) -> Result<Self> {
        let _ = RecognitionConfig::from_config(document, false)?;
        let _ = PostProcessConfig::from_config(document)?;
        let mode = if !document.dictation.enabled {
            Mode::Http
        } else if document.post_process.enabled {
            Mode::Transcription
        } else {
            Mode::Integrated
        };
        let language = document.transcription.language.clone();
        let audio = match mode {
            Mode::Http => {
                let config = crate::transcriber::TranscriberConfig::from_config(document)?;
                let mut meta = config.metadata();
                if let Some(candidate) = candidate {
                    meta.prompt = (!candidate.is_empty()).then(|| candidate.into());
                }
                StageIdentity::new(
                    &meta.endpoint,
                    &meta.model,
                    meta.language,
                    Some(meta.temperature),
                    meta.prompt,
                    "http-multipart-wav-v1",
                )?
            }
            Mode::Integrated | Mode::Transcription => {
                let prompt = candidate.map(str::to_owned).unwrap_or_else(|| {
                    if mode == Mode::Transcription {
                        crate::dictation::ASR_PROMPT.into()
                    } else {
                        document.dictation.prompt()
                    }
                });
                ensure!(
                    prompt.chars().count() <= 4096,
                    "dictation prompt exceeds 4096 characters"
                );
                StageIdentity::new(
                    &document.dictation.url,
                    &document.dictation.model,
                    language.clone(),
                    None,
                    Some(prompt),
                    if mode == Mode::Transcription {
                        crate::dictation::TRANSCRIPTION_INPUT_LAYOUT
                    } else {
                        crate::dictation::INPUT_LAYOUT
                    },
                )?
            }
        };
        let cleanup = if document.post_process.enabled {
            Some(StageIdentity::new(
                document
                    .inference
                    .api
                    .post_process
                    .api_url
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("cleanup URL required"))?,
                document
                    .inference
                    .api
                    .post_process
                    .model
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("cleanup model required"))?,
                None,
                Some(document.post_process.temperature),
                Some(PromptComponents::postprocess_prompt(
                    document.post_process.prompt.as_deref(),
                )),
                "structured-text-contexts-v1",
            )?)
        } else {
            None
        };
        let seed_sha256 = match cases {
            Some(cases) => digest(
                &cases
                    .cases
                    .iter()
                    .map(|case| (&case.id, &case.seed))
                    .collect::<Vec<_>>(),
            )?,
            None => digest(seed)?,
        };
        let mut identity = Self {
            policy: POLICY.into(),
            mode,
            audio,
            cleanup,
            cleanup_preheat: document.post_process.enabled && document.post_process.preheat_enabled,
            candidate_override: candidate.is_some(),
            segmentation: SEGMENTATION.into(),
            pacing: PACING.into(),
            memory_enabled: document.dictation.memory.enabled,
            memory_policy: if cases.is_some() {
                "isolated-explicit-scenario-v1"
            } else {
                "production-snapshot-isolated-copy-v1"
            }
            .into(),
            seed_sha256,
            clock: if cases.is_some() {
                CLOCK
            } else {
                PRODUCTION_CLOCK
            }
            .into(),
            feedback: if cases.is_some() {
                FEEDBACK
            } else {
                PRODUCTION_FEEDBACK
            }
            .into(),
            cases_contract_sha256: cases.map(RealtimeCases::contract_digest).transpose()?,
            sha256: String::new(),
        };
        identity.sha256 = identity.compute_digest()?;
        identity.validate()?;
        Ok(identity)
    }

    fn compute_digest(&self) -> Result<String> {
        let mut value = self.clone();
        value.sha256.clear();
        digest(&value)
    }

    pub fn validate(&self) -> Result<()> {
        self.audio.validate()?;
        if let Some(cleanup) = &self.cleanup {
            cleanup.validate()?;
        }
        ensure!(
            self.policy == POLICY
                && self.segmentation == SEGMENTATION
                && self.pacing == PACING
                && self.clock
                    == if self.cases_contract_sha256.is_some() {
                        CLOCK
                    } else {
                        PRODUCTION_CLOCK
                    }
                && self.feedback
                    == if self.cases_contract_sha256.is_some() {
                        FEEDBACK
                    } else {
                        PRODUCTION_FEEDBACK
                    }
                && self.sha256 == self.compute_digest()?,
            "production execution identity mismatch"
        );
        let expected_layout = match self.mode {
            Mode::Http => "http-multipart-wav-v1",
            Mode::Integrated => crate::dictation::INPUT_LAYOUT,
            Mode::Transcription => crate::dictation::TRANSCRIPTION_INPUT_LAYOUT,
        };
        let url = reqwest::Url::parse(&self.audio.endpoint)?;
        ensure!(
            self.audio.input_layout == expected_layout
                && (self.mode == Mode::Http) == matches!(url.scheme(), "http" | "https")
                && (self.mode == Mode::Http) == self.audio.temperature.is_some()
                && (self.mode != Mode::Transcription || self.cleanup.is_some())
                && (self.mode != Mode::Integrated || self.cleanup.is_none()),
            "stage mode/layout mismatch"
        );
        ensure!(
            !self.cleanup_preheat || self.cleanup.is_some(),
            "disabled cleanup cannot preheat"
        );
        if let Some(cleanup) = &self.cleanup {
            ensure!(
                matches!(
                    reqwest::Url::parse(&cleanup.endpoint)?.scheme(),
                    "http" | "https"
                ) && cleanup.input_layout == "structured-text-contexts-v1"
                    && cleanup.temperature.is_some()
                    && cleanup.language.is_none(),
                "invalid cleanup stage identity"
            );
        }
        ensure!(
            self.memory_policy
                == if self.cases_contract_sha256.is_some() {
                    "isolated-explicit-scenario-v1"
                } else {
                    "production-snapshot-isolated-copy-v1"
                }
                && is_digest(&self.seed_sha256),
            "invalid isolated-memory identity"
        );
        Ok(())
    }

    pub fn stt(&self) -> SttSnapshot {
        SttSnapshot {
            endpoint: self.audio.endpoint.clone(),
            model: self.audio.model.clone(),
            language: self.audio.language.clone(),
            temperature: self.audio.temperature.unwrap_or(0.0),
            prompt: self.audio.prompt.clone(),
            input_layout: (self.mode != Mode::Http).then(|| self.audio.input_layout.clone()),
        }
    }

    /// Ordinary comparisons change only the candidate. Diagnostic scenarios may also vary seeds.
    pub fn compare(&self, prior: &Self) -> Result<Vec<IdentityChange>> {
        self.validate()?;
        prior.validate()?;
        let mut current = self.clone();
        let mut old = prior.clone();
        let mut changes = Vec::new();
        if current.audio.prompt != old.audio.prompt
            || current.candidate_override != old.candidate_override
        {
            ensure!(
                self.candidate_override || prior.candidate_override,
                "prompt changed without an explicit candidate override"
            );
            changes.push(IdentityChange {
                field: "audio.prompt".into(),
                before: serde_json::to_value((&old.audio.prompt, old.candidate_override))?,
                after: serde_json::to_value((&current.audio.prompt, current.candidate_override))?,
            });
        }
        if current.seed_sha256 != old.seed_sha256 {
            ensure!(
                self.cases_contract_sha256.is_some() && prior.cases_contract_sha256.is_some(),
                "comparison requires the same initial production memory; rerun the baseline"
            );
            changes.push(IdentityChange {
                field: "memory.seed_sha256".into(),
                before: prior.seed_sha256.clone().into(),
                after: self.seed_sha256.clone().into(),
            });
        }
        current.audio.prompt = None;
        old.audio.prompt = None;
        current.audio.prompt_sha256.clear();
        old.audio.prompt_sha256.clear();
        current.candidate_override = false;
        old.candidate_override = false;
        current.seed_sha256.clear();
        old.seed_sha256.clear();
        current.sha256.clear();
        old.sha256.clear();
        ensure!(
            current == old,
            "comparison rejects different execution policy, mode, noncandidate stage or replay conditions; rerun a compatible baseline"
        );
        Ok(changes)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdentityChange {
    pub field: String,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
}

pub(super) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(sha256_bytes(&serde_json::to_vec(value)?))
}
fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Cleanup {
    Skipped,
    Disabled,
    Applied,
    EmptyFallback,
    FailedFallback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TextTrace {
    pub at: Option<u64>,
    pub input: Snapshot,
    /// The real structured response, never an invented wrapper around audio-only output.
    pub output: serde_json::Value,
    pub prepared_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SampleExecution {
    pub started_at: u64,
    pub finished_at: u64,
    pub raw_text: String,
    pub final_text: Option<String>,
    pub recognition_error: Option<String>,
    pub warning: Option<String>,
    pub cleanup: Cleanup,
    pub cleanup_error: Option<String>,
    pub playback_ms: u64,
    pub recognition_wait_ms: u64,
    pub cleanup_ms: u64,
    pub asr: Vec<SegmentTrace>,
    pub text: Option<TextTrace>,
    pub before: Snapshot,
    pub after: Snapshot,
    pub feedback: Feedback,
    pub confirmation_available: bool,
    pub feedback_applied: bool,
    pub feedback_at: Option<u64>,
}

impl SampleExecution {
    pub fn failure(&self) -> Option<String> {
        self.recognition_error.clone().or_else(|| {
            self.final_text
                .as_ref()
                .filter(|text| !text.trim().is_empty())
                .is_none()
                .then(|| "recognition produced no final text".into())
        })
    }

    pub fn validate(&self, identity: &Identity) -> Result<()> {
        ensure!(
            self.finished_at >= self.started_at,
            "sample clock moved backwards"
        );
        self.feedback.validate()?;
        ensure!(
            self.feedback_applied
                == (matches!(self.feedback, Feedback::Delivered { .. })
                    && self.confirmation_available),
            "feedback application mismatch"
        );
        ensure!(
            self.feedback_at.is_some() == self.feedback_applied
                && self
                    .feedback_at
                    .is_none_or(|at| (self.started_at..=self.finished_at).contains(&at)),
            "feedback timestamp mismatch"
        );
        ensure!(
            !self.confirmation_available || self.recognition_error.is_none(),
            "incomplete recognition has a confirmation"
        );
        let complete = self.recognition_error.is_none() && !self.raw_text.trim().is_empty();
        match self.cleanup {
            Cleanup::Applied => {
                ensure!(
                    complete && identity.cleanup.is_some() && self.cleanup_error.is_none(),
                    "invalid applied cleanup"
                );
                let trace = self
                    .text
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing cleanup trace"))?;
                ensure!(
                    trace.at.is_some() == identity.memory_enabled
                        && trace
                            .at
                            .is_none_or(|at| (self.started_at..=self.finished_at).contains(&at)),
                    "cleanup input clock outside recording"
                );
                let result = DictationResult::parse_transcript(&trace.output.to_string())?;
                ensure!(
                    !result.text.is_empty()
                        && self.final_text.as_deref() == Some(trace.prepared_text.as_str()),
                    "cleanup final differs from prepared text"
                );
            }
            Cleanup::EmptyFallback | Cleanup::FailedFallback => {
                ensure!(
                    complete
                        && identity.cleanup.is_some()
                        && self.text.is_none()
                        && self.final_text.as_deref() == Some(self.raw_text.as_str())
                        && !self.confirmation_available,
                    "invalid cleanup fallback"
                );
                ensure!(
                    self.cleanup_error.is_some() == (self.cleanup == Cleanup::FailedFallback),
                    "cleanup error/status mismatch"
                );
            }
            Cleanup::Disabled | Cleanup::Skipped => {
                ensure!(
                    self.text.is_none() && self.cleanup_error.is_none() && self.cleanup_ms == 0,
                    "skipped cleanup contains stage evidence"
                );
                ensure!(
                    if self.cleanup == Cleanup::Disabled {
                        complete && identity.cleanup.is_none()
                    } else {
                        !complete
                    },
                    "cleanup skip/status mismatch"
                );
                if identity.mode != Mode::Integrated || !complete {
                    ensure!(
                        self.final_text.as_deref().unwrap_or_default() == self.raw_text,
                        "pass-through final differs from raw text"
                    );
                }
            }
        }
        if identity.mode == Mode::Http {
            ensure!(self.asr.is_empty(), "HTTP result contains Realtime traces");
        } else {
            let mut texts = Vec::new();
            let mut previous = None;
            let mut previous_at = self.started_at;
            let mut end = 0;
            for segment in &self.asr {
                ensure!(
                    previous.is_none_or(|index| segment.index > index)
                        && segment.input_layout == identity.audio.input_layout
                        && segment.instructions == identity.audio.prompt,
                    "audio trace stage identity/order mismatch"
                );
                let start = segment
                    .audio_start_sample
                    .ok_or_else(|| anyhow::anyhow!("missing segment start"))?;
                let next = segment
                    .audio_end_sample
                    .ok_or_else(|| anyhow::anyhow!("missing segment end"))?;
                ensure!(
                    start >= end && next > start,
                    "invalid audio segment boundaries"
                );
                end = next;
                previous = Some(segment.index);
                let result =
                    if identity.mode == Mode::Transcription {
                        ensure!(
                            segment.input.is_none()
                                && segment.prompt_input.is_none()
                                && segment.at.is_none(),
                            "audio-only ASR contains context or a memory-clock read"
                        );
                        DictationResult::parse_plain(segment.output.as_str().ok_or_else(|| {
                            anyhow::anyhow!("audio-only output is not plain text")
                        })?)?
                    } else {
                        ensure!(
                            segment.input.is_some() && segment.prompt_input.is_some(),
                            "integrated trace lacks context input"
                        );
                        let at = segment.at.ok_or_else(|| {
                            anyhow::anyhow!("integrated trace lacks its actual memory timestamp")
                        })?;
                        ensure!(
                            (previous_at..=self.finished_at).contains(&at),
                            "integrated memory timestamps are outside execution or nonmonotonic"
                        );
                        previous_at = at;
                        DictationResult::parse(&segment.output.to_string())?
                    };
                texts.push(result.text);
            }
            // Incomplete inputs still retain the successfully recognized segments.
            ensure!(
                crate::text::merge_texts(&texts, identity.audio.language.clone()) == self.raw_text,
                "raw hypothesis differs from audio trace"
            );
        }
        Ok(())
    }
}

/// One isolated production owner per ordinary run or explicit scenario. The directory outlives both workers;
/// shutdown always happens before the ephemeral state is removed, even after an input failure.
pub(super) struct Runner {
    pipeline: RecognitionPipeline,
    _directory: tempfile::TempDir,
    clock: Clock,
    base: Option<Arc<std::sync::Mutex<PlaybackClock>>>,
}

struct PlaybackClock {
    at: u64,
    anchor: Instant,
    running: bool,
}

impl Runner {
    pub fn new(
        document: &ConfigDocument,
        candidate: Option<String>,
        seed: &MemorySeed,
        at: u64,
    ) -> Result<Self> {
        let base = Arc::new(std::sync::Mutex::new(PlaybackClock {
            at,
            anchor: Instant::now(),
            running: false,
        }));
        let clock_base = Arc::clone(&base);
        let clock: Clock = Arc::new(move || {
            let state = clock_base.lock().unwrap();
            state.at.saturating_add(if state.running {
                state.anchor.elapsed().as_secs()
            } else {
                0
            })
        });
        Self::with_clock(document, candidate, seed, clock, Some(base))
    }

    fn with_clock(
        document: &ConfigDocument,
        candidate: Option<String>,
        seed: &MemorySeed,
        clock: Clock,
        base: Option<Arc<std::sync::Mutex<PlaybackClock>>>,
    ) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("memory.json");
        seed.write(&path, clock())?;
        let mut isolated = document.clone();
        // Ordinary seeds retain the configured vocabulary; only explicit cases replace it.
        isolated.dictation.memory.long_term = seed.long_term.clone();
        let pipeline =
            RecognitionPipeline::from_config(&isolated, Some(path), Arc::clone(&clock), candidate)?;
        Ok(Self {
            pipeline,
            _directory: directory,
            clock,
            base,
        })
    }

    pub fn recognize(
        &self,
        path: &Path,
        id: SessionId,
        advance: u64,
        feedback: Option<&Feedback>,
    ) -> Result<SampleExecution> {
        if let Some(base) = &self.base {
            let at = (self.clock)()
                .checked_add(advance)
                .ok_or_else(|| anyhow::anyhow!("scenario clock overflow"))?;
            *base.lock().unwrap() = PlaybackClock {
                at,
                anchor: Instant::now(),
                running: false,
            };
        }
        let (before, at) = self.pipeline.snapshot_with_time()?;
        let at = at.unwrap_or_else(|| (self.clock)());
        if let Some(base) = &self.base {
            base.lock().unwrap().running = true;
        }
        let outcome = self
            .pipeline
            .recognize_wav(path, id, &CancellationToken::new());
        // Only explicit diagnostics freeze time. Ordinary runs use main's unmodified system clock.
        if let Some(base) = &self.base {
            let finished_at = (self.clock)();
            let mut state = base.lock().unwrap();
            state.at = finished_at;
            state.running = false;
        }
        let mut execution = match outcome {
            Ok(outcome) => self.record_outcome(outcome, at, before, feedback)?,
            Err(_) => SampleExecution {
                started_at: at,
                finished_at: (self.clock)(),
                raw_text: String::new(),
                final_text: None,
                recognition_error: Some("WAV input or recognition initialization failed".into()),
                warning: None,
                cleanup: Cleanup::Skipped,
                cleanup_error: None,
                playback_ms: 0,
                recognition_wait_ms: 0,
                cleanup_ms: 0,
                asr: vec![],
                text: None,
                before,
                after: Snapshot::default(),
                feedback: feedback.cloned().unwrap_or_default(),
                confirmation_available: false,
                feedback_applied: false,
                feedback_at: None,
            },
        };
        let (after, finished_at) = self.pipeline.snapshot_with_time()?;
        execution.after = after;
        execution.finished_at = finished_at.unwrap_or_else(|| (self.clock)());
        Ok(execution)
    }

    fn record_outcome(
        &self,
        mut outcome: RecognitionOutcome,
        at: u64,
        before: Snapshot,
        feedback: Option<&Feedback>,
    ) -> Result<SampleExecution> {
        let (cleanup, cleanup_error) = match outcome.cleanup {
            CleanupStatus::Skipped => (Cleanup::Skipped, None),
            CleanupStatus::Disabled => (Cleanup::Disabled, None),
            CleanupStatus::Applied => (Cleanup::Applied, None),
            CleanupStatus::EmptyFallback => (Cleanup::EmptyFallback, None),
            CleanupStatus::FailedFallback(error) => {
                (Cleanup::FailedFallback, Some(safe_cleanup_error(&error)))
            }
        };
        let final_text = outcome
            .transcript
            .as_ref()
            .map(|transcript| transcript.text.clone());
        // This simulates accepting the displayed final unchanged, never accepting a reference.
        let feedback = feedback.cloned().unwrap_or_else(|| match &final_text {
            Some(text) if outcome.recognition_error.is_none() && !text.trim().is_empty() => {
                Feedback::Delivered { text: text.clone() }
            }
            _ => Feedback::Discard,
        });
        let confirmation = outcome
            .transcript
            .as_mut()
            .and_then(|transcript| transcript.confirmation.take());
        let confirmation_available = confirmation.is_some();
        let feedback_at = match (confirmation, &feedback) {
            (Some(confirmation), Feedback::Delivered { text }) => {
                Some(confirmation.confirm_with_ack(text.clone())?)
            }
            _ => None,
        };
        // All unused handles are dropped before the snapshot barrier (including discard and
        // simulated delivery failure); keeping one alive would block the serial owner forever.
        drop(outcome.transcript);
        Ok(SampleExecution {
            started_at: at,
            finished_at: (self.clock)(),
            raw_text: outcome.raw_text,
            final_text,
            recognition_error: outcome
                .recognition_error
                .as_ref()
                .map(safe_recognition_error),
            warning: outcome.warning,
            cleanup,
            cleanup_error,
            playback_ms: millis(outcome.playback_time),
            recognition_wait_ms: millis(outcome.recognition_wait),
            cleanup_ms: millis(outcome.cleanup_time),
            asr: outcome.asr_traces,
            text: outcome.cleanup_trace.map(|trace| TextTrace {
                at: trace.input_at,
                input: trace.input,
                output: trace.output.raw,
                prepared_text: trace.prepared_text,
            }),
            before,
            after: Snapshot::default(),
            feedback,
            confirmation_available,
            feedback_applied: feedback_at.is_some(),
            feedback_at,
        })
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.pipeline.shutdown();
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
fn safe_cleanup_error(error: &PostProcessError) -> String {
    // Never serialize error Display strings containing provider bodies, URLs or local paths.
    match error {
        PostProcessError::Http(_) => "cleanup transport failure".into(),
        PostProcessError::Json(_) => "invalid cleanup response JSON".into(),
        PostProcessError::InvalidResult => "invalid structured cleanup result".into(),
        PostProcessError::Memory(_) => "cleanup memory unavailable".into(),
        PostProcessError::Api { status } => format!("cleanup API status {status}"),
        PostProcessError::MissingContent => "missing cleanup content".into(),
        PostProcessError::EmptyContent => "empty cleanup content".into(),
    }
}
fn safe_recognition_error(error: &crate::core::orchestrator::SessionError) -> String {
    use crate::core::orchestrator::SessionError;
    match error {
        SessionError::Routing(_) => "recognition session routing failure".into(),
        SessionError::NoChunks => "no audio chunks recorded".into(),
        SessionError::PartialFailure { errors, .. } => {
            format!("partial recognition failure ({} chunks)", errors.len())
        }
        SessionError::InputFailure { .. } => "incomplete audio input".into(),
        SessionError::ConvergenceTimeout { pending_count, .. } => {
            format!("recognition convergence timeout ({pending_count} pending)")
        }
    }
}

pub(super) type Outcomes = HashMap<String, SampleExecution>;

/// Only WAV input and isolated writes differ from main; reuse its state, clock and session owner.
pub(super) fn run_samples(
    document: &ConfigDocument,
    candidate: Option<String>,
    audio: &[(String, PathBuf)],
    seed: &MemorySeed,
) -> Result<Outcomes> {
    let mut outcomes = HashMap::new();
    let runner = Runner::with_clock(
        document,
        candidate,
        seed,
        crate::dictation::system_clock(),
        None,
    )?;
    for (index, (id, path)) in audio.iter().enumerate() {
        let result = runner.recognize(path, SessionId(index as u64 + 1), 0, None)?;
        outcomes.insert(id.clone(), result);
    }
    Ok(outcomes)
}

/// Pure validation uses production memory preparation and feedback, never another model request.
/// Historical auto-learning remains confined to the schema-2 replay contract.
pub(super) fn validate_replay(
    identity: &Identity,
    seed: &MemorySeed,
    executions: &[SampleExecution],
) -> Result<()> {
    let Some(first) = executions.first() else {
        anyhow::bail!("empty production scenario");
    };
    // Plain HTTP without cleanup has no memory owner, even when memory is configured.
    let mut memory = crate::dictation::Replay::production(
        seed,
        first.started_at,
        identity.memory_enabled
            && (identity.mode == Mode::Integrated || identity.cleanup.is_some()),
    );
    for (index, execution) in executions.iter().enumerate() {
        execution.validate(identity)?;
        ensure!(
            execution.before == memory.snapshot(execution.started_at),
            "production input memory differs from replay"
        );
        let id = format!("production-step-{index}");
        let complete =
            execution.recognition_error.is_none() && !execution.raw_text.trim().is_empty();
        if identity.mode == Mode::Integrated {
            // replay(false) validates actual segment input and transient intra-recording learning,
            // but cannot commit any cross-recording state without explicit host feedback.
            let _ = memory.replay(&id, execution.started_at, &execution.asr, false)?;
        }
        let results = if let Some(trace) = &execution.text {
            ensure!(
                trace.input == memory.snapshot(trace.at.unwrap_or(execution.started_at)),
                "cleanup input differs from replayed memory"
            );
            vec![DictationResult::parse_transcript(
                &trace.output.to_string(),
            )?]
        } else if identity.mode == Mode::Integrated && complete {
            execution
                .asr
                .iter()
                .map(|trace| DictationResult::parse(&trace.output.to_string()))
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let confirmation_available = complete
            && (identity.mode == Mode::Integrated
                || (execution.cleanup == Cleanup::Applied && identity.memory_enabled));
        ensure!(
            execution.confirmation_available == confirmation_available,
            "confirmation owner differs from production mode"
        );
        if !results.is_empty() {
            let delivered = if execution.feedback_applied {
                match &execution.feedback {
                    Feedback::Delivered { text } => Some(text.as_str()),
                    _ => None,
                }
            } else {
                None
            };
            let prepared = memory.prepare_and_confirm(
                &id,
                results,
                identity.audio.language.clone(),
                delivered,
                execution.feedback_at.unwrap_or(execution.finished_at),
            );
            ensure!(
                execution.final_text.as_deref() == Some(prepared.as_str()),
                "final text differs from production local correction"
            );
        }
        ensure!(
            execution.after == memory.snapshot(execution.finished_at),
            "feedback memory differs from production replay"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
