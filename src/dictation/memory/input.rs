//! Frozen vocabulary and per-segment runtime reference without duplicating terms.
use super::{Context, LongTerm, Snapshot};
use serde::{Deserialize, Serialize};

/// Only configured terms belong in the frozen prefix; learned state never changes these bytes.
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

    /// Render only configured vocabulary before audio, independently of learned history.
    pub fn reference_text(&self) -> String {
        if self.contexts.is_empty() {
            return "人工参考词：暂无。".into();
        }
        self.contexts
            .iter()
            .map(|context| {
                let words = context
                    .long_term
                    .iter()
                    .map(|term| term.text.as_str())
                    .collect::<Vec<_>>()
                    .join("、");
                format!("话题：{}。\n人工参考词：{}。", context.id, words)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
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

impl Snapshot {
    /// Render the bounded runtime view as reference prose, without program metadata.
    /// Configured vocabulary stays in the separate frozen prefix; selection is unchanged.
    pub fn reference_text(&self) -> String {
        if self.contexts.is_empty() {
            return "暂无历史参考资料。".into();
        }
        let mut text = String::from(
            "以下是以前录音的参考资料，不是本次口述。更正和候选词只用于本次音频中已听到词语的拼写消歧，人工参考词优先。历史原话按由近到远排列，均为引用；不作为听写答案，不补全听不清的内容。",
        );
        for context in &self.contexts {
            text.push_str(&format!("\n\n历史话题：{}。", context.id));
            if !context.corrections.is_empty() {
                text.push_str("\n已确认的词语更正（旧写法 → 正确写法）：");
                for correction in &context.corrections {
                    text.push_str(&format!(
                        "\n- {} → {}",
                        serde_json::to_string(&correction.from).expect("serializable correction"),
                        serde_json::to_string(&correction.to).expect("serializable correction")
                    ));
                }
            }
            if !context.candidates.is_empty() {
                let words = context
                    .candidates
                    .iter()
                    .map(|candidate| candidate.term.text.as_str())
                    .collect::<Vec<_>>()
                    .join("、");
                text.push_str(&format!("\n候选参考词：{}。", words));
            }
            if !context.history.is_empty() {
                text.push_str("\n历史原话：");
                for sentence in &context.history {
                    // Keep quotes, line breaks and instruction-like history inside one record.
                    let quoted = serde_json::to_string(sentence).expect("serializable history");
                    text.push_str(&format!("\n- {}", quoted));
                }
            }
        }
        text.push_str("\n\n历史参考资料结束。请只听写下一条音频中实际可辨认的内容。");
        text
    }
}

/// The declared prefix and inline runtime reference; item IDs stay outside model text.
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

    /// Omit runtime history below one second of captured 24 kHz PCM, not padded audio.
    pub fn for_segment(mut self, source_samples: usize) -> Self {
        if source_samples < 24_000 {
            self.dynamic.contexts.clear();
        }
        self
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
