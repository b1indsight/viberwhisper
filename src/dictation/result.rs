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

/// A model candidate whose source range was validated once at the JSON boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub kind: TermKind,
    pub text: String,
    pub span: Range<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateWire {
    kind: TermKind,
    text: String,
    quote: String,
}

impl CandidateWire {
    /// Resolve one occurrence; the wire quote is not carried into memory or feedback.
    fn locate(self, text: &str) -> Option<Candidate> {
        if !valid_term(&self.text) || self.quote.is_empty() || self.quote.chars().count() > 320 {
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
        word_boundary(text, &span).then_some(Candidate {
            kind: self.kind,
            text: self.text,
            span,
        })
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
    pub fn context_label(&self) -> Option<&str> {
        match &self.context {
            Some(ContextChoice::Existing { id }) => Some(id),
            Some(ContextChoice::New { label }) => Some(label),
            _ => None,
        }
    }

    /// Validate a Realtime segment within its JSON and UTF-8 text budgets.
    pub fn parse(text: &str, context_ids: &[String]) -> Result<Self> {
        ensure!(text.len() <= 64 * 1024, "dictation response is too large");
        let result = Self::parse_transcript(text, context_ids)?;
        ensure!(result.text.len() <= 16_384, "dictation text is too long");
        Ok(result)
    }

    /// Validate a complete HTTP transcript without applying Realtime segment budgets.
    /// Text survives malformed optional metadata; invalid JSON never reaches the input field.
    pub fn parse_transcript(text: &str, context_ids: &[String]) -> Result<Self> {
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
        ensure!(!wire.text.contains('\0'), "invalid text");
        let context = serde_json::from_value::<ContextChoice>(wire.context)
            .ok()
            .filter(|choice| match choice {
                ContextChoice::Existing { id } => context_ids.contains(id),
                ContextChoice::New { label } => valid_label(label),
                ContextChoice::Uncertain => false,
            });
        let candidates = wire
            .memory_candidates
            .as_array()
            .filter(|items| context.is_some() && items.len() <= 8)
            .into_iter()
            .flatten()
            .filter_map(|value| serde_json::from_value::<CandidateWire>(value.clone()).ok())
            .filter_map(|candidate| candidate.locate(&wire.text))
            .collect();
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
