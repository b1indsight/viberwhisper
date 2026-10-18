//! Complete per-segment prompt input, split around audio without duplicating vocabulary.
use super::{Context, LongTerm, Snapshot};
use serde::{Deserialize, Serialize};

/// Only configured terms belong before audio; learned state never changes these bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StablePrefix {
    contexts: Vec<StableContext>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StableContext {
    id: String,
    label: String,
    long_term: Vec<LongTerm>,
}

impl StablePrefix {
    pub(super) fn new(snapshot: Snapshot) -> Self {
        Self {
            contexts: snapshot
                .contexts
                .into_iter()
                .map(|c| StableContext {
                    id: c.id,
                    label: c.label,
                    long_term: c.long_term,
                })
                .collect(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            contexts: self
                .contexts
                .iter()
                .map(|c| Context {
                    id: c.id.clone(),
                    label: c.label.clone(),
                    long_term: c.long_term.clone(),
                    candidates: Vec::new(),
                    corrections: Vec::new(),
                    history: Vec::new(),
                })
                .collect(),
        }
    }
}

/// The exact declared prefix and inline suffix; item IDs are deliberately outside model text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PromptInput {
    pub stable: StablePrefix,
    pub dynamic: Snapshot,
}

impl PromptInput {
    /// Reserve frozen vocabulary first, then trim runtime fields under the shared memory budget.
    /// Both allocation and result validation use the same topic union in `snapshot`.
    pub(super) fn new(stable: StablePrefix, runtime: Snapshot) -> Self {
        let mut input = Self {
            stable,
            dynamic: runtime,
        };
        for context in &mut input.dynamic.contexts {
            context.long_term.clear();
        }
        let mut merged = input.snapshot();
        merged.contexts.truncate(4);
        while !merged.within_budget() {
            if !merged.trim_runtime() {
                // New empty topic envelopes cannot evict an already accepted vocabulary prefix.
                let index = merged
                    .contexts
                    .iter()
                    .rposition(|c| !input.stable.contexts.iter().any(|s| s.id == c.id))
                    .expect("the frozen vocabulary already fits the snapshot budget");
                merged.contexts.remove(index);
            }
        }
        for context in &mut merged.contexts {
            context.long_term.clear();
        }
        merged.contexts.retain(|c| {
            !c.candidates.is_empty()
                || !c.corrections.is_empty()
                || !c.history.is_empty()
                || !input.stable.contexts.iter().any(|s| s.id == c.id)
        });
        input.dynamic = merged;
        input
    }

    /// Reconstruct the bounded business view used for topic validation and learning.
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.stable.snapshot();
        for context in &self.dynamic.contexts {
            if let Some(existing) = snapshot.contexts.iter_mut().find(|c| c.id == context.id) {
                existing.candidates.clone_from(&context.candidates);
                existing.corrections.clone_from(&context.corrections);
                existing.history.clone_from(&context.history);
            } else {
                snapshot.contexts.push(context.clone());
            }
        }
        snapshot
    }
}

#[cfg(test)]
mod tests;
