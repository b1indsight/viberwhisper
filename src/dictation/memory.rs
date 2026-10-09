use std::{fs, io::Write, path::PathBuf};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    config::{LongTerm, MemoryConfig},
    result::{ContextChoice, DictationResult},
};

const DAY: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    recording: String,
    segment: usize,
    at: u64,
    context: String,
    text: String,
    terms: Vec<LongTerm>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryFile {
    schema_version: u32,
    observations: Vec<Observation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct SupportedTerm {
    #[serde(flatten)]
    term: LongTerm,
    supporting_recordings: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct Snapshot {
    pub contexts: Vec<Context>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct Context {
    pub id: String,
    pub label: String,
    pub long_term: Vec<LongTerm>,
    pub candidates: Vec<SupportedTerm>,
    pub history: Vec<String>,
}

impl Snapshot {
    pub fn ids(&self) -> Vec<String> {
        self.contexts.iter().map(|c| c.id.clone()).collect()
    }
    #[cfg(test)]
    pub fn history_is_empty(&self) -> bool {
        self.contexts.iter().all(|c| c.history.is_empty())
    }
}

/// Only observations are persisted. User-owned long-term configuration is never rewritten.
#[derive(Clone)]
pub(super) struct Memory {
    path: Option<PathBuf>,
    settings: MemoryConfig,
    observations: Vec<Observation>,
    frozen: bool,
}

impl Memory {
    pub fn load(path: Option<PathBuf>, settings: MemoryConfig, now: u64) -> Result<Self> {
        let path = path.filter(|_| settings.enabled);
        let mut observations = Vec::new();
        if let Some(path) = &path {
            match fs::metadata(path) {
                Ok(meta) => {
                    ensure!(meta.len() <= 4 * 1024 * 1024, "memory file exceeds limit");
                    let stored: MemoryFile = serde_json::from_slice(&fs::read(path)?)?;
                    ensure!(
                        stored.schema_version == 1,
                        "unsupported dictation memory schema"
                    );
                    observations = stored.observations;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mut memory = Self {
            path,
            settings,
            observations,
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
            frozen: true,
        }
    }

    fn prune(&mut self, now: u64) {
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
        for label in labels.into_iter().take(4) {
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
                history: recent
                    .iter()
                    .take(3)
                    .map(|o| o.text.chars().take(320).collect())
                    .collect(),
            };
            snapshot.contexts.push(context);
        }
        // Trim globally by priority, so one context's history cannot crowd out
        // another context's configured terms. Configuration order breaks ties.
        while serde_json::to_string(&snapshot)
            .expect("serializable snapshot")
            .chars()
            .count()
            > 6000
        {
            if snapshot
                .contexts
                .iter_mut()
                .rev()
                .any(|c| c.history.pop().is_some())
                || snapshot
                    .contexts
                    .iter_mut()
                    .rev()
                    .any(|c| c.candidates.pop().is_some())
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
        snapshot
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
        let label = match &result.context {
            Some(ContextChoice::Existing { id }) => id.clone(),
            Some(ContextChoice::New { label }) => label.clone(),
            _ => return,
        };
        self.observations.push(Observation {
            recording: recording.into(),
            segment,
            at: now,
            context: label.clone(),
            text: result.text.chars().take(1024).collect(),
            terms: result
                .memory_candidates
                .iter()
                .map(|c| LongTerm {
                    context: label.clone(),
                    kind: c.kind.clone(),
                    text: c.text.clone(),
                })
                .collect(),
        });
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
                schema_version: 1,
                observations: self.observations.clone(),
            },
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
}
