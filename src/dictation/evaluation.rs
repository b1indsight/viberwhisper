//! Isolated seeded memory and structured observations through the production recording path.
use super::{
    config::{DictationConfig, LongTerm, MemoryConfig, valid_label, valid_term},
    engine::Recording,
    memory::{Memory, Observation, Snapshot},
    transport::Connection,
};
use crate::core::config::SecretValue;
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
}

impl MemorySeed {
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
        let mut observations_per_context = HashMap::new();
        for o in &self.observations {
            ensure!(
                valid_label(&o.context)
                    && !o.recording.is_empty()
                    && o.recording.len() <= 128
                    && !o.recording.starts_with("sample:")
                    && o.at <= at
                    && o.text.chars().count() <= 1024
                    && !o.text.contains('\0')
                    && o.terms.len() <= 8
                    && o.terms
                        .iter()
                        .all(|t| t.context == o.context && valid_term(&t.text))
                    && origins.insert((o.recording.clone(), o.segment)),
                "invalid seed observation"
            );
            *observations_per_context
                .entry(o.context.clone())
                .or_insert(0usize) += 1;
            counts.entry(o.context.clone()).or_insert(0);
        }
        ensure!(
            counts.len() <= 4
                && observations_per_context.values().all(|n| *n <= 20)
                && serde_json::to_vec(&self.observations)?.len() <= 3 * 1024 * 1024,
            "seed observations exceed memory limits"
        );
        let memory = self.memory(at);
        // Validate the untrimmed visible snapshot: raising the budget here would change production policy.
        let size = serde_json::to_string(&memory.untrimmed_snapshot(at))?
            .chars()
            .count();
        ensure!(size <= 6000, "seed snapshot exceeds memory budget");
        Ok(())
    }

    pub(super) fn memory(&self, at: u64) -> Memory {
        Memory::seeded(
            MemoryConfig {
                enabled: true,
                long_term: self.long_term.clone(),
            },
            self.observations.clone(),
            at,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentTrace {
    pub index: usize,
    pub input: Snapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_input: Option<super::memory::PromptInput>,
    pub output: Value,
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
    pub fn snapshot(&self, at: u64) -> Snapshot {
        self.memory.snapshot(at)
    }

    /// Historical report assertions retain their original context-envelope budget decisions.
    pub fn legacy_snapshot(&self, at: u64) -> Snapshot {
        self.memory.legacy_snapshot(at)
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
            ensure!(
                previous.is_none_or(|index| trace.index > index),
                "segment indices out of order"
            );
            if let Some(input) = &trace.prompt_input {
                ensure!(
                    *input == memory.prompt_input(&stable, at) && trace.input == input.snapshot(),
                    "trace prompt input differs from replayed memory"
                );
            } else {
                ensure!(
                    trace.input == memory.legacy_snapshot(at),
                    "trace input differs from replayed memory"
                );
            }
            let result = super::result::DictationResult::parse(
                &trace.output.to_string(),
                &trace.input.ids(),
            )?;
            memory.learn(&format!("sample:{id}"), trace.index, &result, at);
            previous = Some(trace.index);
        }
        Ok(Self {
            memory: if success { memory } else { self.memory.clone() },
        })
    }
}

/// Owns an ephemeral connection and memory for exactly one explicitly seeded scenario.
pub(crate) struct RealtimeEvaluator {
    settings: DictationConfig,
    key: Option<SecretValue>,
    runtime: tokio::runtime::Runtime,
    memory: Memory,
    connection: Option<Connection>,
}

impl RealtimeEvaluator {
    pub fn new(
        settings: DictationConfig,
        key: Option<SecretValue>,
        seed: &MemorySeed,
        at: u64,
    ) -> Result<Self> {
        settings.validate()?;
        seed.validate(at)?;
        Ok(Self {
            settings,
            key,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
            memory: seed.memory(at),
            connection: None,
        })
    }

    pub fn snapshot(&self, at: u64) -> Snapshot {
        self.memory.snapshot(at)
    }

    /// A failed recording rolls back its memory updates and resets its connection, as the live worker does.
    pub fn transcribe(
        &mut self,
        path: &Path,
        id: &str,
        at: u64,
        language: Option<String>,
    ) -> (Result<String>, Vec<SegmentTrace>) {
        let mut recording = Recording::for_evaluation(format!("sample:{id}"), at);
        let mut memory = self.memory.clone();
        let result = self.runtime.block_on(async {
            let reader = crate::audio::WavChunkReader::open(path)?;
            if self.connection.is_none() {
                self.connection = Some(
                    Connection::connect(&self.settings, self.key.as_ref().map(SecretValue::expose))
                        .await?,
                );
            }
            let connection = self.connection.as_mut().unwrap();
            for frame in reader {
                recording.push(connection, &mut memory, &frame?).await?;
            }
            recording.finish(connection, &mut memory).await?;
            let text = recording.text(language);
            ensure!(!text.is_empty(), "STT returned empty text");
            Ok(text)
        });
        if result.is_ok() {
            self.memory = memory;
        } else {
            self.connection = None;
        }
        (result, recording.traces())
    }
}

pub(crate) use MemoryReplay as Replay;

#[cfg(test)]
mod tests;
