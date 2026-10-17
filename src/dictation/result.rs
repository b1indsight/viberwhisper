use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ops::Range;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use super::config::{TermKind, valid_label, valid_term};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ContextChoice {
    Existing { id: String },
    New { label: String },
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    pub kind: TermKind,
    pub text: String,
    pub quote: String,
}

impl Candidate {
    /// Locate one model-selected occurrence using a verbatim, unique source quote.
    pub fn span(&self, text: &str) -> Option<Range<usize>> {
        if self.quote.is_empty() || self.quote.chars().count() > 320 {
            return None;
        }
        let mut quotes = text
            .char_indices()
            .map(|(at, _)| at)
            .filter(|at| text[*at..].starts_with(&self.quote));
        let start = quotes.next()?;
        if quotes.next().is_some() {
            return None;
        }
        let spans = term_spans(&self.quote, &self.text);
        if spans.len() != 1 {
            return None;
        }
        let span = start + spans[0].start..start + spans[0].end;
        word_boundary(text, &span).then_some(span)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DictationResult {
    pub text: String,
    pub context: Option<ContextChoice>,
    pub memory_candidates: Vec<Candidate>,
    pub raw: Value,
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
        let raw: Value = serde_json::from_str(text)?;
        let wire: Wire = serde_json::from_value(raw.clone())?;
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
            candidates.retain(|c| valid_term(&c.text) && c.span(&wire.text).is_some());
        }
        Ok(Self {
            text: wire.text,
            context,
            memory_candidates: candidates,
            raw,
        })
    }
}

/// Match metadata against delivered text without treating casing or compatibility glyphs as omissions.
pub(super) fn normalized(text: &str) -> String {
    text.nfkc().flat_map(char::to_lowercase).collect()
}

pub(super) fn word_boundary(text: &str, span: &Range<usize>) -> bool {
    let boundary = |at| {
        at == text.len()
            || text
                .split_word_bound_indices()
                .any(|(start, _)| start == at)
    };
    boundary(span.start) && boundary(span.end)
}

/// Match complete Unicode terms while retaining original UTF-8 byte positions.
pub(super) fn term_spans(text: &str, term: &str) -> Vec<Range<usize>> {
    let expected = normalized(term);
    let graphemes: Vec<_> = text.grapheme_indices(true).collect();
    let mut spans = Vec::new();
    for (index, (start, _)) in graphemes.iter().enumerate() {
        let mut value = String::new();
        for (at, grapheme) in graphemes.iter().skip(index).take(80) {
            value.push_str(&normalized(grapheme));
            if value == expected {
                let span = *start..at + grapheme.len();
                if word_boundary(text, &span) {
                    spans.push(span);
                }
                break;
            }
            if !expected.starts_with(&value) {
                break;
            }
        }
    }
    spans
}
