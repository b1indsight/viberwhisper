//! User-confirmed, context-scoped term corrections; model output alone is not evidence.
mod alignment;

use super::{
    config::{LongTerm, TermKind, valid_term},
    result::{Candidate, ContextChoice, DictationResult, normalized},
};
use alignment::Alignment;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Replacement {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Correction {
    pub context: String,
    pub rule: Replacement,
    pub recording: String,
    pub at: u64,
}

struct Applied {
    rule: Replacement,
    shown: Range<usize>,
}

struct Segment {
    source: DictationResult,
    proper_nouns: Vec<Range<usize>>,
    shown: String,
    span: Range<usize>,
    applied: Vec<Applied>,
}

/// Owns the source and displayed revisions until a successful delivery supplies feedback.
pub(super) struct Prepared {
    pub text: String,
    segments: Vec<Segment>,
}

pub(super) struct Confirmed {
    pub results: Vec<(usize, DictationResult)>,
    pub changes: Vec<(String, Replacement)>,
    pub revoked: Vec<(String, String)>,
}

impl Prepared {
    pub fn new(
        results: Vec<DictationResult>,
        rules: &[Correction],
        long_term: &[LongTerm],
        language: Option<String>,
    ) -> Self {
        let mut segments = Vec::new();
        let mut text = String::new();
        let separator = crate::text::segment_separator(language.as_deref());
        for source in results {
            let proper_nouns = proper_noun_spans(&source);
            let (shown, applied) = apply(&source, &proper_nouns, rules, long_term);
            if !text.is_empty() && !shown.is_empty() {
                text.push_str(separator);
            }
            let start = text.len();
            text.push_str(&shown);
            segments.push(Segment {
                source,
                proper_nouns,
                shown,
                span: start..text.len(),
                applied,
            });
        }
        Self { text, segments }
    }

    /// Learn only uniquely aligned whole candidates, and distinguish edits from program output.
    pub fn feedback(&self, final_text: &str) -> Option<Confirmed> {
        if final_text.len() > 64 * 1024 || final_text.contains('\0') || final_text.trim().is_empty()
        {
            return None;
        }
        let alignment = Alignment::new(&self.text, final_text)?;
        let mut confirmed = Confirmed {
            results: vec![],
            changes: vec![],
            revoked: vec![],
        };
        let mut spellings = Vec::new();
        for (index, segment) in self.segments.iter().enumerate() {
            let Some(label) = context(&segment.source) else {
                continue;
            };
            if segment.shown.is_empty()
                || (self.text != final_text
                    && self.segments.len() > 1
                    && self.text.match_indices(&segment.shown).count() != 1)
            {
                continue;
            }
            let Some(span) = alignment.project(segment.span.clone()) else {
                continue;
            };
            let edited = &final_text[span];
            let Some(shown_to_final) = Alignment::new(&segment.shown, edited) else {
                continue;
            };
            for applied in &segment.applied {
                let Some(span) = shown_to_final.project(applied.shown.clone()) else {
                    continue;
                };
                let value = &edited[span];
                spellings.push((
                    label.to_owned(),
                    Replacement {
                        from: applied.rule.from.clone(),
                        to: value.into(),
                    },
                ));
                if value != applied.rule.to {
                    confirmed
                        .revoked
                        .push((label.into(), applied.rule.from.clone()));
                    if valid_replacement(&applied.rule.from, value)
                        && shown_to_final.replaces(&applied.shown)
                    {
                        confirmed.changes.push((
                            label.into(),
                            Replacement {
                                from: applied.rule.from.clone(),
                                to: value.into(),
                            },
                        ));
                    }
                }
            }
            let Some(raw_to_final) = Alignment::new(&segment.source.text, edited) else {
                continue;
            };
            let raw_to_shown = Alignment::new(&segment.source.text, &segment.shown);
            let mut terms = Vec::new();
            for term in &segment.source.memory_candidates {
                let Some(raw_span) = unique_term(&segment.source.text, &term.text) else {
                    if contains_term(edited, &term.text) {
                        terms.push(term.clone());
                    }
                    continue;
                };
                if term.kind == TermKind::ProperNoun && !segment.proper_nouns.contains(&raw_span) {
                    continue;
                }
                let Some(final_span) = raw_to_final.project(raw_span.clone()) else {
                    continue;
                };
                let value = &edited[final_span];
                if !valid_term(value) {
                    continue;
                }
                terms.push(Candidate {
                    kind: term.kind.clone(),
                    text: value.into(),
                });
                let shown_value = raw_to_shown
                    .as_ref()
                    .and_then(|a| a.project(raw_span.clone()))
                    .map(|span| &segment.shown[span]);
                let original = &segment.source.text[raw_span.clone()];
                if term.kind == TermKind::ProperNoun
                    && shown_value.is_some_and(|shown| shown != value)
                    && raw_to_final.replaces(&raw_span)
                    && valid_replacement(original, value)
                {
                    confirmed.changes.push((
                        label.into(),
                        Replacement {
                            from: original.into(),
                            to: value.into(),
                        },
                    ));
                }
            }
            let mut result = segment.source.clone();
            result.text = edited.into();
            result.memory_candidates = terms;
            confirmed.results.push((index, result));
        }
        // Conflicting final spellings of one source in a single recording are
        // evidence to stop applying a rule, not evidence for an arbitrary winner.
        spellings.extend(confirmed.changes.iter().cloned());
        let conflicts: Vec<_> = spellings
            .iter()
            .filter(|(label, rule)| {
                spellings.iter().any(|(other_label, other)| {
                    label == other_label && rule.from == other.from && rule.to != other.to
                })
            })
            .map(|(label, rule)| (label.clone(), rule.from.clone()))
            .collect();
        confirmed
            .changes
            .retain(|(label, rule)| !conflicts.contains(&(label.clone(), rule.from.clone())));
        confirmed.revoked.extend(conflicts);
        Some(confirmed)
    }
}

pub(super) fn context(result: &DictationResult) -> Option<&str> {
    match &result.context {
        Some(ContextChoice::Existing { id }) => Some(id),
        Some(ContextChoice::New { label }) => Some(label),
        _ => None,
    }
}

pub(super) fn valid_replacement(from: &str, to: &str) -> bool {
    from != to
        && valid_term(from)
        && valid_term(to)
        && from.chars().any(char::is_alphabetic)
        && to.chars().any(char::is_alphabetic)
        && from
            .chars()
            .filter(|c| c.is_alphabetic())
            .ne(to.chars().filter(|c| c.is_alphabetic()))
}

fn word_boundary(text: &str, span: &Range<usize>) -> bool {
    let boundary = |at| {
        at == text.len()
            || text
                .split_word_bound_indices()
                .any(|(start, _)| start == at)
    };
    boundary(span.start) && boundary(span.end)
}

fn term_spans(text: &str, term: &str) -> Vec<Range<usize>> {
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

fn unique_term(text: &str, term: &str) -> Option<Range<usize>> {
    let spans = term_spans(text, term);
    if spans.len() == 1 {
        spans.into_iter().next()
    } else {
        None
    }
}

fn contains_term(text: &str, term: &str) -> bool {
    !term_spans(text, term).is_empty()
}

/// Select complete model-extracted names for both learning and application.
/// A containing name owns nested fragments; crossing overlaps are ambiguous.
fn proper_noun_spans(source: &DictationResult) -> Vec<Range<usize>> {
    let mut spans: Vec<_> = source
        .memory_candidates
        .iter()
        .filter(|term| term.kind == TermKind::ProperNoun)
        .flat_map(|term| term_spans(&source.text, &term.text))
        .collect();
    spans.sort_unstable_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));
    spans.dedup();

    let mut selected = Vec::new();
    let mut at = 0;
    while at < spans.len() {
        let first = &spans[at];
        let mut end = first.end;
        let mut next = at + 1;
        while next < spans.len() && spans[next].start < end {
            end = end.max(spans[next].end);
            next += 1;
        }
        if first.end == end {
            selected.push(first.clone());
        }
        at = next;
    }
    selected
}

fn cyclic(rule: &Correction, rules: &[Correction]) -> bool {
    let mut current = rule.rule.to.as_str();
    let mut visited = std::collections::HashSet::new();
    while visited.insert(current) {
        if current == rule.rule.from {
            return true;
        }
        let Some(next) = rules
            .iter()
            .find(|r| r.context == rule.context && r.rule.from == current)
        else {
            return false;
        };
        current = &next.rule.to;
    }
    true
}

fn apply(
    source: &DictationResult,
    proper_nouns: &[Range<usize>],
    rules: &[Correction],
    long_term: &[LongTerm],
) -> (String, Vec<Applied>) {
    let text = &source.text;
    let context = context(source);
    let rules: Vec<_> = rules
        .iter()
        .filter(|r| Some(r.context.as_str()) == context && !cyclic(r, rules))
        .collect();
    if proper_nouns.is_empty() || rules.is_empty() {
        return (text.clone(), Vec::new());
    }
    let protected: Vec<_> = long_term
        .iter()
        .filter(|term| Some(term.context.as_str()) == context)
        .flat_map(|term| term_spans(text, &term.text))
        .collect();
    let (mut output, mut applied) = (String::new(), Vec::new());
    let mut at = 0;
    for span in proper_nouns {
        output.push_str(&text[at..span.start]);
        let rule = rules.iter().copied().find(|r| {
            text[span.clone()] == r.rule.from
                && !protected
                    .iter()
                    .any(|p| p.start < span.end && span.start < p.end)
                && !rules
                    .iter()
                    .any(|other| other.rule.from == r.rule.from && other.rule.to != r.rule.to)
        });
        if let Some(rule) = rule {
            let start = output.len();
            output.push_str(&rule.rule.to);
            applied.push(Applied {
                rule: rule.rule.clone(),
                shown: start..output.len(),
            });
        } else {
            output.push_str(&text[span.clone()]);
        }
        at = span.end;
    }
    output.push_str(&text[at..]);
    (output, applied)
}

#[cfg(test)]
mod tests;
