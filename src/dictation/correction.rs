//! User-confirmed, context-scoped term corrections; model output alone is not evidence.
mod alignment;

use super::{
    config::{LongTerm, TermKind, valid_term},
    memory::Observation,
    result::{DictationResult, term_spans, word_boundary},
};
use alignment::Alignment;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range};

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

struct Term {
    original: String,
    shown: Range<usize>,
    protected: bool,
}

struct Segment {
    context: Option<String>,
    shown: Range<usize>,
    terms: Vec<Term>,
}

/// Owns only the displayed revision and the information needed for confirmation.
pub(super) struct Prepared {
    pub text: String,
    segments: Vec<Segment>,
}

/// One accumulated decision for a source spelling, including unchanged evidence.
pub(super) enum Decision {
    Keep(String),
    Set(String),
    Remove,
}

impl Decision {
    // Conflicts are terminal; equal spellings retain any manual correction evidence.
    fn merge(&mut self, next: &Self) {
        let same = matches!((&*self, next),
            (Self::Keep(a) | Self::Set(a), Self::Keep(b) | Self::Set(b)) if a == b);
        if !same {
            *self = Self::Remove;
        } else if let Self::Set(value) = next {
            *self = Self::Set(value.clone());
        }
    }
}

pub(super) struct Confirmed {
    pub observations: Vec<Observation>,
    pub decisions: BTreeMap<(String, String), Decision>,
}

impl Prepared {
    /// Render once, translating every retained candidate into displayed coordinates.
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
            let (shown, mut terms) = render(&source, rules, long_term);
            if !text.is_empty() && !shown.is_empty() {
                text.push_str(separator);
            }
            let start = text.len();
            text.push_str(&shown);
            for term in &mut terms {
                term.shown = start + term.shown.start..start + term.shown.end;
            }
            segments.push(Segment {
                context: source.context,
                shown: start..text.len(),
                terms,
            });
        }
        Self { text, segments }
    }

    /// One displayed-to-final alignment supplies context, terms and rule decisions.
    pub fn feedback(&self, final_text: &str, recording: &str, now: u64) -> Option<Confirmed> {
        if final_text.len() > 64 * 1024 || final_text.contains('\0') || final_text.trim().is_empty()
        {
            return None;
        }
        let alignment = Alignment::new(&self.text, final_text)?;
        let mut confirmed = Confirmed {
            observations: vec![],
            decisions: BTreeMap::new(),
        };
        let spans: Vec<_> = self.segments.iter().map(|s| s.shown.clone()).collect();
        for (index, segment) in self.segments.iter().enumerate() {
            let Some(label) = &segment.context else {
                continue;
            };
            if segment.shown.is_empty()
                || (self.text != final_text
                    && self.segments.len() > 1
                    && self
                        .text
                        .match_indices(&self.text[segment.shown.clone()])
                        .count()
                        != 1)
            {
                continue;
            }
            let Some(span) = alignment.project_segment(&spans, index) else {
                continue;
            };
            if span.is_empty() {
                continue;
            }
            let mut terms = Vec::new();
            for term in &segment.terms {
                let Some(projected) = alignment.project_noun(term.shown.clone()) else {
                    continue;
                };
                let shown = &self.text[term.shown.clone()];
                let value = name_without_added_sentence_end(shown, &final_text[projected]);
                if !valid_term(value) {
                    continue;
                }
                let remembered = LongTerm {
                    context: label.clone(),
                    kind: TermKind::ProperNoun,
                    text: value.into(),
                };
                if !terms.contains(&remembered) {
                    terms.push(remembered);
                }
                if term.protected {
                    continue;
                }
                let decision = if !valid_replacement(shown, value) {
                    Decision::Keep(shown.into())
                } else if valid_replacement(&term.original, value) {
                    Decision::Set(value.into())
                } else {
                    Decision::Remove
                };
                confirmed
                    .decisions
                    .entry((label.clone(), term.original.clone()))
                    .and_modify(|previous| previous.merge(&decision))
                    .or_insert(decision);
            }
            confirmed.observations.push(Observation::new(
                recording,
                index,
                now,
                label,
                &final_text[span],
                terms,
            ));
        }
        Some(confirmed)
    }
}

fn sentence_end(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | '!' | '?' | ';' | ':' | '。' | '，' | '！' | '？' | '；' | '：' | '、' | '…'
    )
}

/// Newly added sentence endings stay in delivered text, outside the learned name.
/// Punctuation already owned by the model candidate and symbols such as C++ stay intact.
fn name_without_added_sentence_end<'a>(shown: &str, value: &'a str) -> &'a str {
    if shown.ends_with(sentence_end) {
        value
    } else {
        value.trim_end_matches(|c: char| c.is_whitespace() || sentence_end(c))
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

/// Apply known replacements backwards so original source positions stay valid.
/// The parser supplies disjoint complete names in source order.
fn render(
    source: &DictationResult,
    rules: &[Correction],
    long_term: &[LongTerm],
) -> (String, Vec<Term>) {
    let label = source.context.as_deref();
    let protected: Vec<_> = long_term
        .iter()
        .filter(|term| Some(term.context.as_str()) == label)
        .flat_map(|term| term_spans(&source.text, &term.text))
        .collect();
    let mut terms: Vec<_> = source
        .memory_candidates
        .iter()
        .map(|term| Term {
            original: source.text[term.span.clone()].into(),
            shown: term.span.clone(),
            protected: protected
                .iter()
                .any(|p| p.start < term.span.end && term.span.start < p.end),
        })
        .collect();
    let mut output = source.text.clone();
    for candidate in source.memory_candidates.iter().rev() {
        let span = &candidate.span;
        let Some(rule) = rules.iter().find(|rule| {
            Some(rule.context.as_str()) == label
                && source.text[span.clone()] == rule.rule.from
                && !protected
                    .iter()
                    .any(|p| p.start < span.end && span.start < p.end)
                && !rules.iter().any(|other| {
                    other.context == rule.context
                        && other.rule.from == rule.rule.from
                        && other.rule.to != rule.rule.to
                })
                && !cyclic(rule, rules)
        }) else {
            continue;
        };
        let to = &rule.rule.to;
        output.replace_range(span.clone(), to);
        for term in &mut terms {
            if span.end <= term.shown.start {
                term.shown.start = term.shown.start - span.len() + to.len();
                term.shown.end = term.shown.end - span.len() + to.len();
            } else if term.shown == *span {
                term.shown.end = span.start + to.len();
            }
        }
    }
    (output, terms)
}

#[cfg(test)]
mod tests;
