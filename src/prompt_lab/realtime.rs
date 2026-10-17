//! Explicit, isolated Realtime scenarios and reproducible local assertions.
use super::dataset::sha256_bytes;
use crate::{
    core::config::SecretValue,
    dictation::{
        Candidate, ContextChoice, DictationResult, MemorySeed, RealtimeEvaluator, Replay,
        SegmentTrace, Snapshot,
        config::{DictationConfig, TermKind, valid_label, valid_term},
    },
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub(super) const POLICY: &str = "realtime-context-memory-v3";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RealtimeCases {
    pub schema_version: u32,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    pub id: String,
    pub at: u64,
    #[serde(default)]
    pub seed: MemorySeed,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub sample_id: String,
    #[serde(default)]
    pub advance_seconds: u64,
    #[serde(default)]
    pub expect: Expectations,
}

impl RealtimeCases {
    pub fn read(path: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 4 * 1024 * 1024,
            "Realtime cases must be a regular JSON file of at most 4 MiB"
        );
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    /// Complete, unique coverage keeps the existing report's metric weights and review IDs stable.
    pub fn validate(&self, ready: &[String]) -> Result<()> {
        ensure!(
            self.schema_version == 1 && !self.cases.is_empty() && self.cases.len() <= ready.len(),
            "invalid Realtime case schema or case count"
        );
        let mut cases = HashSet::new();
        let mut samples = HashSet::new();
        for case in &self.cases {
            ensure!(
                valid_label(&case.id) && cases.insert(&case.id) && !case.steps.is_empty(),
                "invalid or duplicate Realtime case ID, or empty steps"
            );
            case.seed.validate(case.at)?;
            let mut at = case.at;
            for step in &case.steps {
                ensure!(
                    samples.insert(step.sample_id.clone()) && ready.contains(&step.sample_id),
                    "Realtime steps must reference every ready sample exactly once"
                );
                at = at
                    .checked_add(step.advance_seconds)
                    .ok_or_else(|| anyhow::anyhow!("scenario clock overflow"))?;
                step.expect.validate()?;
            }
        }
        ensure!(
            samples.len() == ready.len(),
            "Realtime cases do not cover every ready sample"
        );
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        Ok(sha256_bytes(&serde_json::to_vec(self)?))
    }

    /// Seeds are experimental inputs; ordering, clocks and assertions define the comparison contract.
    pub fn contract_digest(&self) -> Result<String> {
        let mut contract = self.clone();
        for case in &mut contract.cases {
            case.seed = MemorySeed::default();
        }
        contract.digest()
    }
}

/// Semantic term assertions stay independent of per-response quote provenance,
/// which is checked separately by each segment's metadata validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateExpectation {
    kind: TermKind,
    text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SnapshotExpectations {
    pub contexts_present: Vec<String>,
    pub contexts_absent: Vec<String>,
    pub terms_present: Vec<TermExpectation>,
    pub terms_absent: Vec<TermExpectation>,
    pub history_present: Vec<String>,
    pub history_absent: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TermExpectation {
    pub context: String,
    pub kind: TermKind,
    pub text: String,
    #[serde(default)]
    pub supporting_recordings: Option<usize>,
}

impl SnapshotExpectations {
    fn validate(&self) -> Result<()> {
        for strings in [
            &self.contexts_present,
            &self.contexts_absent,
            &self.history_present,
            &self.history_absent,
        ] {
            ensure!(
                strings.len() <= 32
                    && strings
                        .iter()
                        .all(|s| !s.is_empty() && s.chars().count() <= 320 && !s.contains('\0')),
                "invalid snapshot assertions"
            );
        }
        for terms in [&self.terms_present, &self.terms_absent] {
            ensure!(
                terms.len() <= 32
                    && terms.iter().all(|t| valid_label(&t.context)
                        && valid_term(&t.text)
                        && t.supporting_recordings.is_none_or(|n| n > 0 && n <= 20)),
                "invalid term assertions"
            );
        }
        Ok(())
    }

    fn check(&self, snapshot: &Snapshot, prefix: &str, checks: &mut Vec<Check>) {
        let has_term = |term: &TermExpectation| {
            snapshot.contexts.iter().any(|c| {
                c.id == term.context
                    && (term.supporting_recordings.is_none()
                        && c.long_term
                            .iter()
                            .any(|t| t.kind == term.kind && t.text == term.text)
                        || c.candidates.iter().any(|t| {
                            t.term.kind == term.kind
                                && t.term.text == term.text
                                && term
                                    .supporting_recordings
                                    .is_none_or(|n| t.supporting_recordings == n)
                        }))
            })
        };
        for (present, values) in [
            (true, &self.contexts_present),
            (false, &self.contexts_absent),
        ] {
            for v in values {
                checks.push(Check::new(
                    format!("{prefix}.context:{v}"),
                    snapshot.contexts.iter().any(|c| c.id == *v) == present,
                ));
            }
        }
        for (present, values) in [(true, &self.terms_present), (false, &self.terms_absent)] {
            for v in values {
                checks.push(Check::new(
                    format!("{prefix}.term:{}:{}:{:?}", v.context, v.text, v.kind),
                    has_term(v) == present,
                ));
            }
        }
        for (present, values) in [(true, &self.history_present), (false, &self.history_absent)] {
            for v in values {
                checks.push(Check::new(
                    format!("{prefix}.history:{v}"),
                    snapshot
                        .contexts
                        .iter()
                        .any(|c| c.history.iter().any(|h| h.contains(v)))
                        == present,
                ));
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Expectations {
    pub input: SnapshotExpectations,
    pub context: Option<ContextChoice>,
    pub candidates_present: Vec<CandidateExpectation>,
    pub candidates_absent: Vec<CandidateExpectation>,
    pub memory: SnapshotExpectations,
    pub forbidden_text: Vec<String>,
}

impl Expectations {
    fn validate(&self) -> Result<()> {
        self.input.validate()?;
        self.memory.validate()?;
        if let Some(context) = &self.context {
            ensure!(
                match context {
                    ContextChoice::Existing { id } => valid_label(id),
                    ContextChoice::New { label } => valid_label(label),
                    ContextChoice::Uncertain => true,
                },
                "invalid context assertion"
            );
        }
        for values in [&self.candidates_present, &self.candidates_absent] {
            ensure!(
                values.len() <= 32 && values.iter().all(|c| valid_term(&c.text)),
                "invalid candidate assertions"
            );
        }
        ensure!(
            self.forbidden_text.len() <= 32
                && self
                    .forbidden_text
                    .iter()
                    .all(|s| !s.is_empty() && s.chars().count() <= 320 && !s.contains('\0')),
            "invalid forbidden text assertion"
        );
        Ok(())
    }

    pub fn check(
        &self,
        before: &Snapshot,
        segments: &[SegmentTrace],
        after: &Snapshot,
        text: &str,
    ) -> Vec<Check> {
        let mut checks = Vec::new();
        self.input.check(before, "input", &mut checks);
        self.memory.check(after, "memory", &mut checks);
        let mut candidates = Vec::new();
        for trace in segments {
            let parsed = DictationResult::parse(&trace.output.to_string(), &trace.input.ids());
            let raw_context =
                serde_json::from_value::<ContextChoice>(trace.output["context"].clone());
            let raw_candidates =
                serde_json::from_value::<Vec<Candidate>>(trace.output["memory_candidates"].clone());
            let valid = parsed.as_ref().is_ok_and(|p| {
                raw_context.as_ref().is_ok_and(|context| match context {
                    ContextChoice::Uncertain => raw_candidates.as_ref().is_ok_and(Vec::is_empty),
                    _ => {
                        p.context.as_ref() == Some(context)
                            && !matches!(context, ContextChoice::New { label } if trace.input.ids().contains(label))
                            && raw_candidates
                                .as_ref()
                                .is_ok_and(|c| *c == p.memory_candidates)
                    }
                })
            });
            checks.push(Check::new(
                format!("segment.{}.metadata", trace.index),
                valid,
            ));
            if let Ok(p) = parsed {
                candidates.extend(p.memory_candidates);
            }
        }
        if let Some(expected) = &self.context {
            let actual = segments.last().and_then(|s| {
                serde_json::from_value::<ContextChoice>(s.output["context"].clone()).ok()
            });
            checks.push(Check::new(
                "context".into(),
                actual.as_ref() == Some(expected),
            ));
        }
        for (present, values) in [
            (true, &self.candidates_present),
            (false, &self.candidates_absent),
        ] {
            for v in values {
                checks.push(Check::new(
                    format!("candidate:{:?}:{}", v.kind, v.text),
                    candidates
                        .iter()
                        .any(|c| c.kind == v.kind && c.text == v.text)
                        == present,
                ));
            }
        }
        for v in &self.forbidden_text {
            checks.push(Check::new(format!("forbidden_text:{v}"), !text.contains(v)));
        }
        checks
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    pub name: String,
    pub passed: bool,
}
impl Check {
    fn new(name: String, passed: bool) -> Self {
        Self { name, passed }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepResult {
    pub case_id: String,
    pub sample_id: String,
    pub at: u64,
    pub before: Snapshot,
    pub segments: Vec<SegmentTrace>,
    pub after: Snapshot,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RealtimeReport {
    pub policy_version: String,
    pub cases: RealtimeCases,
    pub cases_sha256: String,
    pub contract_sha256: String,
    pub steps: Vec<StepResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeedChange {
    pub case_id: String,
    pub before: MemorySeed,
    pub after: MemorySeed,
}

impl RealtimeReport {
    pub fn seed_changes(&self, prior: &Self) -> Vec<SeedChange> {
        self.cases
            .cases
            .iter()
            .zip(&prior.cases.cases)
            .filter(|(current, old)| current.seed != old.seed)
            .map(|(current, old)| SeedChange {
                case_id: current.id.clone(),
                before: old.seed.clone(),
                after: current.seed.clone(),
            })
            .collect()
    }

    pub fn validate_seed_changes(&self, changes: &[SeedChange], changed: bool) -> Result<()> {
        ensure!(
            changed != changes.is_empty(),
            "Realtime seed-change marker mismatch"
        );
        let mut seen = HashSet::new();
        for change in changes {
            let case = self
                .cases
                .cases
                .iter()
                .find(|case| case.id == change.case_id)
                .ok_or_else(|| anyhow::anyhow!("unknown changed case"))?;
            ensure!(
                seen.insert(&change.case_id)
                    && case.seed == change.after
                    && change.before != change.after,
                "invalid Realtime seed change"
            );
            change.before.validate(case.at)?;
        }
        Ok(())
    }
}

pub(crate) struct RealtimeRun {
    pub cases: RealtimeCases,
    pub settings: DictationConfig,
    pub key: Option<SecretValue>,
}

impl RealtimeReport {
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|s| s.checks.iter().all(|c| c.passed))
    }

    /// Recompute traces, learning, expectations and derived checks before trusting a stored report.
    pub fn validate(
        &self,
        samples: &[super::regression::SampleResult],
        language: Option<String>,
    ) -> Result<()> {
        self.cases.validate(
            &samples
                .iter()
                .map(|s| s.sample_id.clone())
                .collect::<Vec<_>>(),
        )?;
        ensure!(
            [
                POLICY,
                "realtime-context-memory-v1",
                "realtime-context-memory-v2"
            ]
            .contains(&self.policy_version.as_str())
                && self.cases_sha256 == self.cases.digest()?
                && self.contract_sha256 == self.cases.contract_digest()?,
            "Realtime report identity mismatch"
        );
        let mut rows = self.steps.iter();
        for case in &self.cases.cases {
            let mut memory = Replay::new(&case.seed, case.at);
            let mut at = case.at;
            for step in &case.steps {
                at += step.advance_seconds;
                let row = rows
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing Realtime step"))?;
                let sample = samples
                    .iter()
                    .find(|s| s.sample_id == step.sample_id)
                    .unwrap();
                ensure!(
                    row.case_id == case.id
                        && row.sample_id == step.sample_id
                        && row.at == at
                        && row.before == memory.snapshot(at),
                    "Realtime step input mismatch"
                );
                memory =
                    memory.replay(&step.sample_id, at, &row.segments, sample.error.is_none())?;
                ensure!(
                    row.after == memory.snapshot(at),
                    "Realtime step memory mismatch"
                );
                if let Some(text) = &sample.hypothesis {
                    let texts = row
                        .segments
                        .iter()
                        .map(|t| t.output["text"].as_str().unwrap_or_default().to_string())
                        .collect::<Vec<_>>();
                    ensure!(
                        crate::text::merge_texts(&texts, language.clone()) == *text,
                        "Realtime trace text mismatch"
                    );
                }
                let checks = step.expect.check(
                    &row.before,
                    &row.segments,
                    &row.after,
                    sample.hypothesis.as_deref().unwrap_or_default(),
                );
                ensure!(
                    row.checks == checks,
                    "Realtime checks differ from recorded evidence"
                );
            }
        }
        ensure!(rows.next().is_none(), "unexpected Realtime steps");
        Ok(())
    }
}

pub(crate) type Outcomes = HashMap<String, std::result::Result<String, String>>;

/// Execute only audio and explicit seed state; reference text and assertions never reach the recognizer.
pub(crate) fn run(
    run: RealtimeRun,
    audio: &HashMap<String, PathBuf>,
    language: Option<String>,
) -> Result<(RealtimeReport, Outcomes)> {
    let mut outcomes = HashMap::new();
    let mut steps = Vec::new();
    for case in &run.cases.cases {
        let mut evaluator =
            RealtimeEvaluator::new(run.settings.clone(), run.key.clone(), &case.seed, case.at)?;
        let mut at = case.at;
        for step in &case.steps {
            at += step.advance_seconds;
            let before = evaluator.snapshot(at);
            let (result, segments) = evaluator.transcribe(
                &audio[&step.sample_id],
                &step.sample_id,
                at,
                language.clone(),
            );
            let after = evaluator.snapshot(at);
            let checks = step.expect.check(
                &before,
                &segments,
                &after,
                result.as_ref().map(String::as_str).unwrap_or_default(),
            );
            steps.push(StepResult {
                case_id: case.id.clone(),
                sample_id: step.sample_id.clone(),
                at,
                before,
                segments,
                after,
                checks,
            });
            outcomes.insert(step.sample_id.clone(), result.map_err(|e| format!("{e:#}")));
        }
    }
    Ok((
        RealtimeReport {
            policy_version: POLICY.into(),
            cases_sha256: run.cases.digest()?,
            contract_sha256: run.cases.contract_digest()?,
            cases: run.cases,
            steps,
        },
        outcomes,
    ))
}

#[cfg(test)]
mod tests;
