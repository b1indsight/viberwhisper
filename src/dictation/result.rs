use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::{TermKind, valid_label, valid_term};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ContextChoice {
    Existing { id: String },
    New { label: String },
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Candidate {
    pub kind: TermKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(super) struct DictationResult {
    pub text: String,
    pub context: Option<ContextChoice>,
    pub memory_candidates: Vec<Candidate>,
}

impl DictationResult {
    /// Text survives malformed optional metadata; invalid JSON never reaches the input field.
    pub fn parse(text: &str, context_ids: &[String]) -> Result<Self> {
        ensure!(text.len() <= 64 * 1024, "dictation response is too large");
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: String,
            #[serde(default)]
            context: Value,
            #[serde(default)]
            memory_candidates: Value,
        }
        let wire: Wire = serde_json::from_str(text)?;
        ensure!(wire.text.len() <= 16_384, "dictation text is too long");
        ensure!(!wire.text.contains('\0'), "invalid text");
        let context = serde_json::from_value::<ContextChoice>(wire.context)
            .ok()
            .filter(|choice| match choice {
                ContextChoice::Existing { id } => context_ids.contains(id),
                ContextChoice::New { label } => valid_label(label),
                ContextChoice::Uncertain => false,
            });
        let mut candidates =
            serde_json::from_value::<Vec<Candidate>>(wire.memory_candidates).unwrap_or_default();
        if context.is_none() || candidates.len() > 8 {
            candidates.clear();
        } else {
            candidates.retain(|c| valid_term(&c.text));
        }
        Ok(Self {
            text: wire.text,
            context,
            memory_candidates: candidates,
        })
    }
}
