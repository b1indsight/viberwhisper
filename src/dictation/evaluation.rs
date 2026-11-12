//! Isolated seeded memory and structured observations through the production recording path.
use super::{
    config::{DictationConfig, LongTerm, MemoryConfig},
    correction::Correction,
    memory::{Memory, Observation, Snapshot},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MemorySeed {
    pub long_term: Vec<LongTerm>,
    pub observations: Vec<Observation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<Correction>,
}

impl MemorySeed {
    /// Read an explicit production file, retaining canonical state and the original vocabulary.
    /// Disabled memory skips filesystem reads; failed reads recover to ephemeral state as in main.
    /// No source path is retained and no source bytes are written or renamed.
    pub fn capture(path: &Path, settings: MemoryConfig, at: u64) -> Self {
        Memory::load_or_ephemeral(Some(path.to_owned()), settings, at).into_seed()
    }

    /// Initialize an isolated destination using production retention and persistence.
    /// Artificial scenarios must call `validate` separately; captured state may exceed visible limits.
    /// Long-term vocabulary is configuration, so the host must install it separately.
    pub fn write(&self, path: &Path, at: u64) -> Result<()> {
        let mut memory = self.memory(at);
        memory.set_seed_path(path.to_owned());
        memory.save()
    }

    /// Reject seeds that production would silently discard or truncate before the first request.
    pub fn validate(&self, at: u64) -> Result<()> {
        let settings = DictationConfig {
            memory: MemoryConfig {
                enabled: true,
                long_term: self.long_term.clone(),
            },
            ..Default::default()
        };
        settings.validate()?;
        let mut counts = HashMap::new();
        let mut origins = HashSet::new();
        for t in &self.long_term {
            *counts.entry(t.context.clone()).or_insert(0usize) += 1;
        }
        ensure!(
            counts.len() <= 4 && counts.values().all(|n| *n <= 16),
            "seed vocabulary exceeds snapshot limits"
        );
        for o in &self.observations {
            ensure!(
                !o.recording.is_empty()
                    && o.recording.len() <= 128
                    && !o.recording.starts_with("sample:")
                    && !o.text.contains('\0')
                    && origins.insert((o.recording.clone(), o.segment)),
                "invalid seed observation"
            );
            counts.entry(o.context.clone()).or_insert(0);
        }
        for correction in &self.corrections {
            counts.entry(correction.context.clone()).or_insert(0);
        }
        ensure!(counts.len() <= 4, "seed contexts exceed snapshot limits");
        let memory = self.memory(at);
        // Validate the untrimmed visible snapshot: raising the budget here would change production policy.
        let size = serde_json::to_string(&memory.untrimmed_snapshot(at))?
            .chars()
            .count();
        ensure!(size <= 6000, "seed snapshot exceeds memory budget");
        // Production owns retention, including expiration, correction deduplication and per-context limits.
        let retained = memory.into_seed();
        ensure!(
            retained.observations.len() == self.observations.len(),
            "seed observations would be discarded by production memory retention"
        );
        ensure!(
            retained.corrections.len() == self.corrections.len(),
            "seed corrections would be discarded by production memory retention"
        );
        Ok(())
    }

    pub(super) fn memory(&self, at: u64) -> Memory {
        Memory::seeded(
            MemoryConfig {
                enabled: true,
                long_term: self.long_term.clone(),
            },
            self.observations.clone(),
            self.corrections.clone(),
            at,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentTrace {
    pub index: usize,
    /// Absent only for pure ASR; never synthesize structured metadata for audio-only input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_input: Option<super::memory::PromptInput>,
    #[serde(default = "integrated_layout")]
    pub input_layout: String,
    /// Actual Unix-seconds prompt time; historical traces use the recording's supplied time.
    /// Pure ASR omits it because that stage never reads the memory clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// Legacy traces did not retain instructions or 24 kHz source-audio boundaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_start_sample: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_end_sample: Option<usize>,
    /// Integrated: actual JSON object. Transcription: verified plain-text string.
    pub output: Value,
}

fn integrated_layout() -> String {
    super::INPUT_LAYOUT.into()
}

/// Replays recording outputs using production memory, without any file or network access.
pub(crate) struct MemoryReplay {
    memory: Memory,
}

impl MemoryReplay {
    pub fn new(seed: &MemorySeed, at: u64) -> Self {
        Self {
            memory: seed.memory(at),
        }
    }

    /// Recompute production preparation/feedback with the actual configured memory switch.
    /// This is pure validation, with no connection, file or alternative recognizer.
    pub fn production(seed: &MemorySeed, at: u64, enabled: bool) -> Self {
        Self {
            memory: Memory::seeded(
                MemoryConfig {
                    enabled,
                    long_term: seed.long_term.clone(),
                },
                seed.observations.clone(),
                seed.corrections.clone(),
                at,
            ),
        }
    }

    /// Return the pre-edit preview; only independently supplied delivered text commits feedback.
    pub fn prepare_and_confirm(
        &mut self,
        id: &str,
        results: Vec<super::result::DictationResult>,
        language: Option<String>,
        delivered: Option<&str>,
        at: u64,
    ) -> String {
        let prepared = self.memory.prepare(results, language);
        if let Some(text) = delivered {
            self.memory.confirm(id, &prepared, text, at);
        }
        prepared.text
    }

    pub fn snapshot(&self, at: u64) -> Snapshot {
        self.memory.snapshot(at)
    }

    pub fn replay(
        &self,
        id: &str,
        at: u64,
        traces: &[SegmentTrace],
        success: bool,
    ) -> Result<Self> {
        let mut memory = self.memory.clone();
        let stable = memory.stable_prefix();
        let mut previous = None;
        for trace in traces {
            let at = trace.at.unwrap_or(at);
            ensure!(
                previous.is_none_or(|index| trace.index > index),
                "segment indices out of order"
            );
            let input = trace.prompt_input.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Realtime trace lacks the current prompt input; rerun evaluation")
            })?;
            ensure!(
                trace.input_layout == super::INPUT_LAYOUT
                    && *input == memory.prompt_input(&stable, at)
                    && trace.input.as_ref() == Some(&input.snapshot()),
                "trace prompt input differs from replayed memory"
            );
            let result = super::result::DictationResult::parse(&trace.output.to_string())?;
            memory.learn(&format!("sample:{id}"), trace.index, &result, at);
            previous = Some(trace.index);
        }
        Ok(Self {
            memory: if success { memory } else { self.memory.clone() },
        })
    }
}

pub(crate) use MemoryReplay as Replay;

#[cfg(test)]
mod tests;
