use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, ops::Range};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use super::{
    UNKNOWN_CONTEXT,
    config::{valid_label, valid_term},
};

const MAX_CANDIDATES: usize = 8;

/// A model candidate whose source range was validated once at the JSON boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub text: String,
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct DictationResult {
    pub text: String,
    pub context: Option<String>,
    pub memory_candidates: Vec<Candidate>,
    /// Protocol validity is separate from whether a name is safe to correct or learn.
    pub metadata_valid: bool,
    pub raw: Value,
}

impl DictationResult {
    /// Validate a Realtime segment within its JSON and UTF-8 text budgets.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= 64 * 1024, "dictation response is too large");
        let result = Self::parse_transcript(text)?;
        ensure!(result.text.len() <= 16_384, "dictation text is too long");
        Ok(result)
    }

    /// Validate a complete HTTP transcript without applying Realtime segment budgets.
    /// Text survives malformed optional metadata; invalid JSON never reaches the input field.
    pub fn parse_transcript(text: &str) -> Result<Self> {
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
        let context = wire
            .context
            .as_str()
            .filter(|label| *label != UNKNOWN_CONTEXT && valid_label(label))
            .map(str::to_owned);
        let items = wire
            .memory_candidates
            .as_array()
            .filter(|items| context.is_some() && items.len() <= MAX_CANDIDATES);
        let names = items
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|name| valid_term(name))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let (candidates, prefix_valid) = locate_candidates(&wire.text, &names);
        let metadata_valid = if wire.context.as_str() == Some(UNKNOWN_CONTEXT) {
            wire.memory_candidates.as_array().is_some_and(Vec::is_empty)
        } else {
            items.is_some_and(|items| items.len() == names.len()) && prefix_valid
        };
        Ok(Self {
            text: wire.text,
            context,
            memory_candidates: candidates,
            metadata_valid,
            raw,
        })
    }
}

/// Locate all mentions of a supplied name, or none when its occurrence count is ambiguous.
/// Complete names own nested fragments even if their own group cannot be accepted.
/// A valid first-eight prefix can still contain groups that are unsafe to correct or learn.
fn locate_candidates(text: &str, names: &[String]) -> (Vec<Candidate>, bool) {
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for name in names {
        groups.entry(normalized(name)).or_default().push(name);
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by_key(|(name, _)| std::cmp::Reverse(name.chars().count()));

    let mut located: Vec<Vec<Range<usize>>> = Vec::new();
    for (_, names) in &groups {
        let mut spans = term_spans(text, names[0]);
        for outer in &located {
            // Both lists are in source order: each owner is visited at most once.
            let (mut next, mut reach) = (0, 0);
            spans.retain(|span| {
                while next < outer.len() && outer[next].start <= span.start {
                    reach = reach.max(outer[next].end);
                    next += 1;
                }
                span.end > reach
            });
        }
        located.push(spans);
    }

    let mut overlapping = vec![false; located.len()];
    for (index, spans) in located.iter().enumerate() {
        overlapping[index] |= spans.windows(2).any(|pair| pair[0].end > pair[1].start);
        for (other_index, other) in located.iter().enumerate().skip(index + 1) {
            let mut next = 0;
            if spans.iter().any(|span| {
                while next < other.len() && other[next].end <= span.start {
                    next += 1;
                }
                other.get(next).is_some_and(|other| other.start < span.end)
            }) {
                overlapping[index] = true;
                overlapping[other_index] = true;
            }
        }
    }

    // Each group contributes at most eight positions to the global first eight.
    // Never sort all transcript occurrences just to validate this bounded prefix.
    let mut prefix: Vec<_> = located
        .iter()
        .enumerate()
        .flat_map(|(group, spans)| {
            spans
                .iter()
                .take(MAX_CANDIDATES)
                .map(move |span| (group, span))
        })
        .collect();
    prefix.sort_by_key(|(_, span)| span.start);
    let prefix_valid = !overlapping.contains(&true)
        && prefix.len() >= names.len()
        && (names.len() == MAX_CANDIDATES || prefix.len() == names.len())
        && names
            .iter()
            .zip(&prefix)
            .all(|(name, (group, _))| normalized(name) == groups[*group].0);

    let mut candidates = Vec::new();
    for (index, ((_, names), spans)) in groups.iter().zip(&located).enumerate() {
        if names.len() != spans.len() || overlapping[index] {
            continue;
        }
        candidates.extend(names.iter().zip(spans).map(|(name, span)| Candidate {
            text: (*name).into(),
            span: span.clone(),
        }));
    }
    candidates.sort_by_key(|candidate| candidate.span.start);
    (candidates, prefix_valid)
}

/// Match metadata against delivered text without treating casing or compatibility glyphs as omissions.
pub(super) fn normalized(text: &str) -> String {
    text.nfkc().flat_map(char::to_lowercase).collect()
}

fn word_bounds(text: &str) -> Vec<bool> {
    let mut bounds = vec![false; text.len() + 1];
    for (at, _) in text.split_word_bound_indices() {
        bounds[at] = true;
    }
    bounds[text.len()] = true;
    bounds
}

pub(super) fn word_boundary(text: &str, span: &Range<usize>) -> bool {
    let bounds = word_bounds(text);
    bounds[span.start] && bounds[span.end]
}

/// Match complete Unicode terms while retaining original UTF-8 byte positions.
pub(super) fn term_spans(text: &str, term: &str) -> Vec<Range<usize>> {
    let expected = normalized(term);
    let bounds = word_bounds(text);
    let graphemes: Vec<_> = text.grapheme_indices(true).collect();
    let mut spans = Vec::new();
    for (index, (start, _)) in graphemes.iter().enumerate() {
        if !bounds[*start] {
            continue;
        }
        let mut value = String::new();
        for (at, grapheme) in graphemes.iter().skip(index).take(80) {
            value.push_str(&normalized(grapheme));
            if value == expected {
                let span = *start..at + grapheme.len();
                if bounds[span.end] {
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
