//! User-confirmed, context-scoped term corrections; model output alone is not evidence.
mod alignment;

use super::{
    config::{LongTerm, TermKind, valid_term},
    result::{Candidate, ContextChoice, DictationResult, normalized},
};
use alignment::Alignment;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};
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

struct Noun {
    raw: Range<usize>,
    shown: Range<usize>,
    protected: bool,
}

struct Segment {
    source: DictationResult,
    proper_nouns: Vec<Noun>,
    shown: String,
    span: Range<usize>,
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
    /// Render model candidates while retaining their exact source and preview positions.
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
            let (shown, proper_nouns) =
                apply(&source, &proper_noun_spans(&source), rules, long_term);
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
            });
        }
        Self { text, segments }
    }

    /// Learn from displayed noun spans; unchanged occurrences also detect spelling conflicts.
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
        let mut spellings = BTreeMap::new();
        let mut conflicts = BTreeSet::new();
        let projected_segments: Vec<_> = self
            .segments
            .iter()
            .enumerate()
            .filter_map(|(index, segment)| {
                if segment.shown.is_empty() {
                    return None;
                }
                let span = alignment.project_segment(segment.span.clone())?;
                (!span.is_empty()).then_some(((index, segment), span))
            })
            .collect();
        for (position, ((index, segment), span)) in projected_segments.iter().enumerate() {
            if !uniquely_owned(&projected_segments, position) {
                continue;
            }
            let Some(label) = context(&segment.source) else {
                continue;
            };
            if self.text != final_text
                && self.segments.len() > 1
                && self.text.match_indices(&segment.shown).count() != 1
            {
                continue;
            }
            let edited = &final_text[span.clone()];
            let Some(shown_to_final) = Alignment::new(&segment.shown, edited) else {
                continue;
            };
            let mut terms = Vec::new();
            let mut remember = |kind, value: &str| {
                if valid_term(value) && terms.len() < 8 {
                    let term = Candidate {
                        kind,
                        text: value.into(),
                    };
                    if !terms.contains(&term) {
                        terms.push(term);
                    }
                }
            };
            let projected: Vec<_> = segment
                .proper_nouns
                .iter()
                .filter_map(|noun| {
                    let span = shown_to_final.project_noun(noun.shown.clone(), edited)?;
                    (!span.is_empty()).then_some((noun, span))
                })
                .collect();
            for (index, (noun, span)) in projected.iter().enumerate() {
                if !uniquely_owned(&projected, index) {
                    continue;
                }
                let original = &segment.source.text[noun.raw.clone()];
                let shown = &segment.shown[noun.shown.clone()];
                let value = name_without_added_sentence_end(shown, &edited[span.clone()]);
                if !valid_term(value) {
                    // Deleting a mention supplies no spelling evidence.
                    continue;
                }
                remember(TermKind::ProperNoun, value);
                if noun.protected {
                    continue;
                }
                let spelling_changed = valid_replacement(shown, value);
                let spelling = if spelling_changed { value } else { shown };
                let key = (label.to_owned(), original.to_owned());
                if spellings
                    .insert(key.clone(), spelling.to_owned())
                    .is_some_and(|previous| previous != spelling)
                {
                    conflicts.insert(key);
                }
                if spelling_changed {
                    if original != shown {
                        confirmed.revoked.push((label.into(), original.into()));
                    }
                    if valid_replacement(original, value) {
                        confirmed.changes.push((
                            label.into(),
                            Replacement {
                                from: original.into(),
                                to: value.into(),
                            },
                        ));
                    }
                }
            }
            if let Some(raw_to_final) = Alignment::new(&segment.source.text, edited) {
                for term in segment
                    .source
                    .memory_candidates
                    .iter()
                    .filter(|term| term.kind == TermKind::Phrase)
                {
                    let Some(raw_span) = unique_term(&segment.source.text, &term.text) else {
                        if contains_term(edited, &term.text) {
                            remember(TermKind::Phrase, &term.text);
                        }
                        continue;
                    };
                    if let Some(span) = raw_to_final.project(raw_span) {
                        remember(TermKind::Phrase, &edited[span]);
                    }
                }
            }
            let mut result = segment.source.clone();
            result.text = edited.into();
            result.memory_candidates = terms;
            confirmed.results.push((*index, result));
        }
        // Conflicting final spellings of one source in a single recording are
        // evidence to stop applying a rule, not evidence for an arbitrary winner.
        confirmed
            .changes
            .retain(|(label, rule)| !conflicts.contains(&(label.clone(), rule.from.clone())));
        confirmed.revoked.extend(conflicts);
        Some(confirmed)
    }
}

/// Alignment is monotone: overlapping neighboring projections have no unique owner.
fn uniquely_owned<T>(projected: &[(T, Range<usize>)], at: usize) -> bool {
    let span = &projected[at].1;
    (at == 0 || projected[at - 1].1.end <= span.start)
        && projected
            .get(at + 1)
            .is_none_or(|(_, next)| span.end <= next.start)
}

/// Newly added sentence endings stay in delivered text, outside the learned name.
/// Punctuation already owned by the model candidate and symbols such as C++ stay intact.
fn name_without_added_sentence_end<'a>(shown: &str, value: &'a str) -> &'a str {
    let sentence_end = |c| {
        matches!(
            c,
            '.' | ','
                | '!'
                | '?'
                | ';'
                | ':'
                | '。'
                | '，'
                | '！'
                | '？'
                | '；'
                | '：'
                | '、'
                | '…'
        )
    };
    if shown.ends_with(sentence_end) {
        value
    } else {
        value.trim_end_matches(|c: char| c.is_whitespace() || sentence_end(c))
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

/// Render each complete noun once and record its actual output range for feedback.
fn apply(
    source: &DictationResult,
    proper_nouns: &[Range<usize>],
    rules: &[Correction],
    long_term: &[LongTerm],
) -> (String, Vec<Noun>) {
    let text = &source.text;
    let context = context(source);
    let rules: Vec<_> = rules
        .iter()
        .filter(|r| Some(r.context.as_str()) == context && !cyclic(r, rules))
        .collect();
    if proper_nouns.is_empty() {
        return (text.clone(), Vec::new());
    }
    let protected_spans: Vec<_> = long_term
        .iter()
        .filter(|term| Some(term.context.as_str()) == context)
        .flat_map(|term| term_spans(text, &term.text))
        .collect();
    let (mut output, mut nouns) = (String::new(), Vec::new());
    let mut at = 0;
    for span in proper_nouns {
        output.push_str(&text[at..span.start]);
        let protected = protected_spans
            .iter()
            .any(|p| p.start < span.end && span.start < p.end);
        let rule = rules.iter().copied().find(|r| {
            !protected
                && text[span.clone()] == r.rule.from
                && !rules
                    .iter()
                    .any(|other| other.rule.from == r.rule.from && other.rule.to != r.rule.to)
        });
        let start = output.len();
        if let Some(rule) = rule {
            output.push_str(&rule.rule.to);
        } else {
            output.push_str(&text[span.clone()]);
        }
        nouns.push(Noun {
            raw: span.clone(),
            shown: start..output.len(),
            protected,
        });
        at = span.end;
    }
    output.push_str(&text[at..]);
    (output, nouns)
}

#[cfg(test)]
mod tests;
