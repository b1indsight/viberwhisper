use std::{fs, io::Write, path::PathBuf};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    UNKNOWN_CONTEXT,
    config::{LongTerm, MemoryConfig, TermKind},
    correction::{Correction, Decision, Prepared, Replacement},
    result::DictationResult,
};

const DAY: u64 = 86_400;

mod input;
pub(crate) use input::{PromptInput, StablePrefix};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    pub recording: String,
    pub segment: usize,
    pub at: u64,
    pub context: String,
    pub text: String,
    pub terms: Vec<LongTerm>,
}

impl Observation {
    /// Build bounded memory content without inventing model-response metadata.
    pub fn new(
        recording: &str,
        segment: usize,
        at: u64,
        context: &str,
        text: &str,
        terms: Vec<LongTerm>,
    ) -> Self {
        Self {
            recording: recording.into(),
            segment,
            at,
            context: context.into(),
            text: text.chars().take(1024).collect(),
            terms,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryFile {
    schema_version: u32,
    observations: Vec<Observation>,
    #[serde(default)]
    corrections: Vec<Correction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SupportedTerm {
    #[serde(flatten)]
    pub term: LongTerm,
    pub supporting_recordings: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub contexts: Vec<Context>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub id: String,
    pub label: String,
    pub long_term: Vec<LongTerm>,
    pub candidates: Vec<SupportedTerm>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<Replacement>,
    pub history: Vec<String>,
}

impl Snapshot {
    #[cfg(test)]
    pub fn ids(&self) -> Vec<String> {
        self.contexts.iter().map(|c| c.id.clone()).collect()
    }
    #[cfg(test)]
    pub fn history_is_empty(&self) -> bool {
        self.contexts.iter().all(|c| c.history.is_empty())
    }

    fn within_budget(&self) -> bool {
        serde_json::to_string(self)
            .expect("serializable snapshot")
            .chars()
            .count()
            <= 6000
    }

    /// Discard lower-priority runtime content globally before touching configured terms.
    fn trim_runtime(&mut self) -> bool {
        self.contexts
            .iter_mut()
            .rev()
            .any(|c| c.history.pop().is_some())
            || self
                .contexts
                .iter_mut()
                .rev()
                .any(|c| c.candidates.pop().is_some())
            || self
                .contexts
                .iter_mut()
                .rev()
                .any(|c| c.corrections.pop().is_some())
    }
}

/// Only observations are persisted. User-owned long-term configuration is never rewritten.
#[derive(Clone)]
pub(super) struct Memory {
    path: Option<PathBuf>,
    settings: MemoryConfig,
    observations: Vec<Observation>,
    corrections: Vec<Correction>,
    frozen: bool,
}

impl Memory {
    pub fn load(path: Option<PathBuf>, settings: MemoryConfig, now: u64) -> Result<Self> {
        let path = path.filter(|_| settings.enabled);
        let mut observations = Vec::new();
        let mut corrections = Vec::new();
        if let Some(path) = &path {
            match fs::metadata(path) {
                Ok(meta) => {
                    ensure!(meta.len() <= 4 * 1024 * 1024, "memory file exceeds limit");
                    let stored: MemoryFile = serde_json::from_slice(&fs::read(path)?)?;
                    ensure!(
                        matches!(stored.schema_version, 1 | 2),
                        "unsupported dictation memory schema"
                    );
                    observations = stored.observations;
                    if stored.schema_version == 2 {
                        corrections = stored.corrections;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mut memory = Self {
            path,
            settings,
            observations,
            corrections,
            frozen: false,
        };
        memory.prune(now);
        Ok(memory)
    }

    pub fn frozen(settings: MemoryConfig) -> Self {
        Self {
            path: None,
            settings,
            observations: Vec::new(),
            corrections: Vec::new(),
            frozen: true,
        }
    }

    /// Build isolated evaluation state; seed validation belongs to the explicit evaluation boundary.
    pub(super) fn seeded(settings: MemoryConfig, observations: Vec<Observation>, at: u64) -> Self {
        let mut memory = Self {
            path: None,
            settings,
            observations,
            corrections: Vec::new(),
            frozen: false,
        };
        memory.prune(at);
        memory
    }

    pub(super) fn observation_count(&self) -> usize {
        self.observations.len()
    }

    fn prune(&mut self, now: u64) {
        self.corrections.retain(|c| {
            super::config::valid_label(&c.context)
                && super::correction::valid_replacement(&c.rule.from, &c.rule.to)
                && c.recording.len() <= 256
        });
        self.corrections.sort_by_key(|c| c.at);
        let mut keys = std::collections::HashSet::new();
        self.corrections.reverse();
        self.corrections
            .retain(|c| keys.insert((c.context.clone(), c.rule.from.clone())));
        self.corrections.truncate(512);
        self.corrections.reverse();
        self.observations.retain(|o| {
            now >= o.at
                && now - o.at < DAY
                && o.text.chars().count() <= 1024
                && o.terms.len() <= 8
                && super::config::valid_label(&o.context)
                && o.terms
                    .iter()
                    .all(|t| t.context == o.context && super::config::valid_term(&t.text))
        });
        let mut contexts = Vec::new();
        let mut counts = std::collections::HashMap::new();
        let mut bytes = 0usize;
        self.observations.reverse();
        self.observations.retain(|o| {
            let size = serde_json::to_vec(o)
                .expect("serializable observation")
                .len();
            if bytes.saturating_add(size) > 3 * 1024 * 1024 {
                return false;
            }
            if !contexts.contains(&o.context) {
                if contexts.len() >= 32 {
                    return false;
                }
                contexts.push(o.context.clone());
            }
            let count = counts.entry(o.context.clone()).or_insert(0);
            *count += 1;
            if *count > 20 {
                return false;
            }
            bytes += size;
            true
        });
        self.observations.reverse();
    }

    pub fn snapshot(&self, now: u64) -> Snapshot {
        self.prompt_input(&self.stable_prefix(), now).snapshot()
    }

    /// Reproduce the pre-layout-v4 budget policy when validating historical reports.
    pub(super) fn legacy_snapshot(&self, now: u64) -> Snapshot {
        let mut snapshot = self.untrimmed_snapshot(now);
        Self::trim_snapshot(&mut snapshot);
        snapshot
    }

    /// Freeze configured vocabulary independently of runtime history and its expiry.
    pub fn stable_prefix(&self) -> StablePrefix {
        StablePrefix::new(Self::frozen(self.settings.clone()).legacy_snapshot(0))
    }

    /// Fill the remaining prompt budget without changing the already declared prefix.
    pub fn prompt_input(&self, stable: &StablePrefix, now: u64) -> PromptInput {
        PromptInput::new(stable.clone(), self.untrimmed_snapshot(now))
    }

    fn usable_corrections(&self) -> impl DoubleEndedIterator<Item = &Correction> {
        self.corrections.iter().filter(|c| {
            !self
                .settings
                .long_term
                .iter()
                .any(|term| term.context == c.context && term.text == c.rule.from)
        })
    }

    pub(super) fn untrimmed_snapshot(&self, now: u64) -> Snapshot {
        if !self.settings.enabled {
            return Snapshot {
                contexts: Vec::new(),
            };
        }
        let mut labels = Vec::new();
        // Reserve scarce context slots for manually configured vocabulary first.
        for term in &self.settings.long_term {
            if !labels.contains(&term.context) {
                labels.push(term.context.clone());
            }
        }
        for correction in self.usable_corrections().rev() {
            if !labels.contains(&correction.context) {
                labels.push(correction.context.clone());
            }
        }
        for o in self
            .observations
            .iter()
            .rev()
            .filter(|o| now >= o.at && now - o.at < DAY)
        {
            if !labels.contains(&o.context) {
                labels.push(o.context.clone());
            }
        }
        let mut snapshot = Snapshot {
            contexts: Vec::new(),
        };
        for label in labels
            .into_iter()
            .filter(|label| label != UNKNOWN_CONTEXT)
            .take(4)
        {
            let recent: Vec<_> = self
                .observations
                .iter()
                .rev()
                .filter(|o| o.context == label && now >= o.at && now - o.at < DAY)
                .collect();
            let mut candidates = Vec::new();
            for o in &recent {
                for term in &o.terms {
                    if candidates.len() < 64 && !candidates.contains(term) {
                        candidates.push(term.clone());
                    }
                }
            }
            let mut candidates: Vec<_> = candidates
                .into_iter()
                .map(|term| {
                    let recordings: std::collections::HashSet<_> = recent
                        .iter()
                        .filter(|o| o.terms.contains(&term))
                        .map(|o| &o.recording)
                        .collect();
                    SupportedTerm {
                        term,
                        supporting_recordings: recordings.len(),
                    }
                })
                .collect();
            candidates.sort_by_key(|term| std::cmp::Reverse(term.supporting_recordings));
            candidates.truncate(16);
            let context = Context {
                id: label.clone(),
                label: label.clone(),
                long_term: self
                    .settings
                    .long_term
                    .iter()
                    .filter(|t| t.context == label)
                    .take(16)
                    .cloned()
                    .collect(),
                candidates,
                corrections: self
                    .usable_corrections()
                    .rev()
                    .filter(|c| c.context == label)
                    .take(16)
                    .map(|c| c.rule.clone())
                    .collect(),
                history: recent
                    .iter()
                    .take(3)
                    .map(|o| o.text.chars().take(320).collect())
                    .collect(),
            };
            snapshot.contexts.push(context);
        }
        snapshot
    }

    fn trim_snapshot(snapshot: &mut Snapshot) {
        // Trim globally by priority, so one context's history cannot crowd out
        // another context's configured terms. Configuration order breaks ties.
        while !snapshot.within_budget() {
            if snapshot.trim_runtime()
                || snapshot
                    .contexts
                    .iter_mut()
                    .rev()
                    .any(|c| c.long_term.pop().is_some())
            {
                continue;
            }
            snapshot.contexts.pop();
        }
    }

    pub fn learn(&mut self, recording: &str, segment: usize, result: &DictationResult, now: u64) {
        if self.frozen || !self.settings.enabled || result.text.trim().is_empty() {
            return;
        }
        self.prune(now);
        if self
            .observations
            .iter()
            .any(|o| o.recording == recording && o.segment == segment)
        {
            return;
        }
        let Some(label) = result.context.as_deref() else {
            return;
        };
        self.observations.push(Observation::new(
            recording,
            segment,
            now,
            label,
            &result.text,
            result
                .memory_candidates
                .iter()
                .map(|c| LongTerm {
                    context: label.into(),
                    kind: TermKind::ProperNoun,
                    text: c.text.clone(),
                })
                .collect(),
        ));
        self.prune(now);
    }

    /// Prepare a reversible preview without changing persistent or cross-recording memory.
    pub fn prepare(&self, results: Vec<DictationResult>, language: Option<String>) -> Prepared {
        let rules: Vec<_> = self
            .usable_corrections()
            .filter(|_| self.settings.enabled)
            .cloned()
            .collect();
        Prepared::new(results, &rules, &self.settings.long_term, language)
    }

    /// Commit one successful delivery. Repeated recording feedback is idempotent.
    pub fn confirm(&mut self, recording: &str, prepared: &Prepared, text: &str, now: u64) {
        if self.frozen
            || !self.settings.enabled
            || self.observations.iter().any(|o| o.recording == recording)
            || self.corrections.iter().any(|c| c.recording == recording)
        {
            return;
        }
        let Some(confirmed) = prepared.feedback(text, recording, now) else {
            return;
        };
        for ((context, from), decision) in confirmed.decisions {
            if matches!(decision, Decision::Keep(_)) {
                continue;
            }
            self.corrections
                .retain(|c| c.context != context || c.rule.from != from);
            if let Decision::Set(to) = decision {
                self.corrections.push(Correction {
                    context,
                    rule: Replacement { from, to },
                    recording: recording.into(),
                    at: now,
                });
            }
        }
        self.observations.extend(confirmed.observations);
        self.prune(now);
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("memory path has no parent"))?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(
            &mut file,
            &MemoryFile {
                schema_version: 2,
                observations: self.observations.clone(),
                corrections: self.corrections.clone(),
            },
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
}
