//! Explicit, isolated Realtime scenarios and reproducible local assertions.
use super::dataset::sha256_bytes;
use crate::dictation::{
    DictationResult, MemorySeed, Replay, SegmentTrace, Snapshot,
    config::{TermKind, valid_label, valid_term},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub(super) const POLICY: &str = "realtime-context-memory-v5";
const CASES_SCHEMA_VERSION: u32 = 3;

fn validate_cases_version(version: Option<u64>) -> Result<()> {
    ensure!(
        version == Some(u64::from(CASES_SCHEMA_VERSION)),
        "unsupported Realtime cases schema_version {version:?}; expected {CASES_SCHEMA_VERSION}; migrate stage-specific assertions and supply explicit delivered feedback (omitted feedback is discard), then rerun production evaluation"
    );
    Ok(())
}

/// Check versions before legacy nested assertions can obscure the migration error during decoding.
pub(super) fn validate_report_version(
    policy: Option<&str>,
    cases_schema: Option<u64>,
) -> Result<()> {
    if policy == Some(super::production::POLICY) {
        return validate_cases_version(cases_schema);
    }
    ensure!(
        policy == Some(POLICY),
        "unsupported Realtime report policy {policy:?}; expected {POLICY}; migrate Realtime cases to schema {CASES_SCHEMA_VERSION} and rerun evaluation instead of rescoring old reports"
    );
    ensure!(
        cases_schema == Some(2),
        "unsupported historical Realtime cases schema_version {cases_schema:?}; {POLICY} requires 2; migrate to cases schema {CASES_SCHEMA_VERSION} and rerun production evaluation"
    );
    Ok(())
}

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
    #[serde(default, skip_serializing_if = "Feedback::is_discard")]
    pub feedback: Feedback,
}

/// Simulated host feedback is independent of reference text and never changes the scored final.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Feedback {
    #[default]
    Discard,
    DeliveryFailed,
    Delivered {
        text: String,
    },
}

impl Feedback {
    fn is_discard(&self) -> bool {
        matches!(self, Self::Discard)
    }
    pub(super) fn validate(&self) -> Result<()> {
        if let Self::Delivered { text } = self {
            ensure!(
                !text.trim().is_empty() && text.len() <= 64 * 1024 && !text.contains('\0'),
                "invalid explicit delivered feedback text"
            );
        }
        Ok(())
    }
}

impl RealtimeCases {
    pub fn read(path: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 4 * 1024 * 1024,
            "Realtime cases must be a regular JSON file of at most 4 MiB"
        );
        let bytes = fs::read(path)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        validate_cases_version(value["schema_version"].as_u64())?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Complete, unique coverage keeps the existing report's metric weights and review IDs stable.
    pub fn validate(&self, ready: &[String]) -> Result<()> {
        ensure!(
            matches!(self.schema_version, 2 | 3),
            "unsupported Realtime cases schema_version; migrate to schema 3 and rerun"
        );
        ensure!(
            !self.cases.is_empty() && self.cases.len() <= ready.len(),
            "invalid Realtime case count"
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
                step.feedback.validate()?;
                if self.schema_version == 3 {
                    step.expect.validate_production()?;
                } else {
                    ensure!(
                        step.feedback == Feedback::Discard
                            && step.expect.audio.is_none()
                            && step.expect.cleanup.is_none()
                            && step.expect.raw_text.is_none()
                            && step.expect.final_text.is_none(),
                        "historical cases cannot contain production feedback/stage assertions"
                    );
                }
            }
        }
        ensure!(
            samples.len() == ready.len(),
            "Realtime cases do not cover every ready sample"
        );
        Ok(())
    }

    pub fn validate_new(&self, ready: &[String]) -> Result<()> {
        validate_cases_version(Some(u64::from(self.schema_version)))?;
        self.validate(ready)
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
    pub context: Option<String>,
    pub candidates_present: Vec<String>,
    pub candidates_absent: Vec<String>,
    pub memory: SnapshotExpectations,
    pub forbidden_text: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<StageExpectations>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<StageExpectations>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StageExpectations {
    pub input: SnapshotExpectations,
    pub audio_only: Option<bool>,
    pub context: Option<String>,
    pub candidates_present: Vec<String>,
    pub candidates_absent: Vec<String>,
    pub forbidden_text: Vec<String>,
}

impl Expectations {
    fn validate(&self) -> Result<()> {
        self.input.validate()?;
        self.memory.validate()?;
        if let Some(context) = &self.context {
            ensure!(valid_label(context), "invalid context assertion");
        }
        for values in [&self.candidates_present, &self.candidates_absent] {
            ensure!(
                values.len() <= 32 && values.iter().all(|c| valid_term(c)),
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

    fn validate_production(&self) -> Result<()> {
        ensure!(
            self.input == SnapshotExpectations::default()
                && self.context.is_none()
                && self.candidates_present.is_empty()
                && self.candidates_absent.is_empty(),
            "schema 3 assertions must name audio or cleanup stage, not legacy input/context/candidates"
        );
        for stage in [&self.audio, &self.cleanup].into_iter().flatten() {
            stage.input.validate()?;
            let legacy = Expectations {
                context: stage.context.clone(),
                candidates_present: stage.candidates_present.clone(),
                candidates_absent: stage.candidates_absent.clone(),
                forbidden_text: stage.forbidden_text.clone(),
                ..Default::default()
            };
            legacy.validate()?;
        }
        ensure!(
            self.cleanup
                .as_ref()
                .is_none_or(|stage| stage.audio_only.is_none()),
            "audio_only is an audio-stage assertion, not a cleanup assertion"
        );
        for text in [&self.raw_text, &self.final_text].into_iter().flatten() {
            ensure!(
                text.len() <= 64 * 1024 && !text.contains('\0'),
                "invalid stage text assertion"
            );
        }
        Ok(())
    }

    pub(super) fn check_production(
        &self,
        execution: &super::production::SampleExecution,
    ) -> Vec<Check> {
        let mut checks = Vec::new();
        self.memory.check(&execution.after, "memory", &mut checks);
        for (name, expected, actual) in [
            (
                "raw_text",
                &self.raw_text,
                Some(execution.raw_text.as_str()),
            ),
            (
                "final_text",
                &self.final_text,
                execution.final_text.as_deref(),
            ),
        ] {
            if let Some(expected) = expected {
                checks.push(Check::new(name.into(), actual == Some(expected.as_str())));
            }
        }
        if let Some(expected) = &self.audio {
            let audio_only = execution.asr.iter().all(|trace| {
                trace.input.is_none() && trace.prompt_input.is_none() && trace.output.is_string()
            });
            if let Some(value) = expected.audio_only {
                checks.push(Check::new(
                    "audio.audio_only".into(),
                    !execution.asr.is_empty() && audio_only == value,
                ));
            }
            for trace in &execution.asr {
                if let Some(input) = &trace.input {
                    expected.input.check(
                        input,
                        &format!("audio.{}.input", trace.index),
                        &mut checks,
                    );
                } else if expected.input != SnapshotExpectations::default() {
                    checks.push(Check::new(
                        format!("audio.{}.context_input_available", trace.index),
                        false,
                    ));
                }
            }
            check_stage(
                expected,
                "audio",
                execution.asr.iter().map(|trace| &trace.output),
                &execution.raw_text,
                &mut checks,
            );
        }
        if let Some(expected) = &self.cleanup {
            if let Some(trace) = &execution.text {
                expected
                    .input
                    .check(&trace.input, "cleanup.input", &mut checks);
                check_stage(
                    expected,
                    "cleanup",
                    std::iter::once(&trace.output),
                    trace.output["text"].as_str().unwrap_or_default(),
                    &mut checks,
                );
            } else {
                checks.push(Check::new("cleanup.executed".into(), false));
            }
        }
        for value in &self.forbidden_text {
            checks.push(Check::new(
                format!("final.forbidden_text:{value}"),
                !execution
                    .final_text
                    .as_deref()
                    .unwrap_or_default()
                    .contains(value),
            ));
        }
        checks
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
            let parsed = DictationResult::parse(&trace.output.to_string());
            let valid = parsed.as_ref().is_ok_and(|p| p.metadata_valid);
            checks.push(Check::new(
                format!("segment.{}.metadata", trace.index),
                valid,
            ));
            if let Ok(p) = parsed {
                candidates.extend(p.memory_candidates);
            }
        }
        if let Some(expected) = &self.context {
            let actual = segments.last().and_then(|s| s.output["context"].as_str());
            checks.push(Check::new(
                "context".into(),
                actual == Some(expected.as_str()),
            ));
        }
        for (present, values) in [
            (true, &self.candidates_present),
            (false, &self.candidates_absent),
        ] {
            for v in values {
                checks.push(Check::new(
                    format!("candidate:{v}"),
                    candidates.iter().any(|c| c.text == *v) == present,
                ));
            }
        }
        for v in &self.forbidden_text {
            checks.push(Check::new(format!("forbidden_text:{v}"), !text.contains(v)));
        }
        checks
    }
}

fn check_stage<'a>(
    expected: &StageExpectations,
    name: &str,
    outputs: impl Iterator<Item = &'a serde_json::Value>,
    text: &str,
    checks: &mut Vec<Check>,
) {
    let outputs = outputs.collect::<Vec<_>>();
    let mut candidates = Vec::new();
    for (index, output) in outputs.iter().enumerate() {
        if output.is_object() {
            let result = DictationResult::parse_transcript(&output.to_string());
            checks.push(Check::new(
                format!("{name}.{index}.metadata"),
                result.as_ref().is_ok_and(|result| result.metadata_valid),
            ));
            if let Ok(result) = result {
                candidates.extend(result.memory_candidates);
            }
        }
    }
    if let Some(context) = &expected.context {
        checks.push(Check::new(
            format!("{name}.context"),
            outputs.last().and_then(|value| value["context"].as_str()) == Some(context.as_str()),
        ));
    }
    for (present, values) in [
        (true, &expected.candidates_present),
        (false, &expected.candidates_absent),
    ] {
        for value in values {
            checks.push(Check::new(
                format!("{name}.candidate:{value}"),
                candidates.iter().any(|candidate| candidate.text == *value) == present,
            ));
        }
    }
    for value in &expected.forbidden_text {
        checks.push(Check::new(
            format!("{name}.forbidden_text:{value}"),
            !text.contains(value),
        ));
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

#[cfg(test)]
pub(crate) struct RealtimeRun {
    pub cases: RealtimeCases,
    pub settings: crate::dictation::config::DictationConfig,
}

impl RealtimeReport {
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|s| s.checks.iter().all(|c| c.passed))
    }

    /// Recompute traces, learning, expectations and derived checks before trusting a stored report.
    pub(super) fn validate(
        &self,
        samples: &[super::regression::SampleResult],
        language: Option<String>,
        identity: Option<&super::production::Identity>,
    ) -> Result<()> {
        validate_report_version(
            Some(&self.policy_version),
            Some(u64::from(self.cases.schema_version)),
        )?;
        self.cases.validate(
            &samples
                .iter()
                .map(|s| s.sample_id.clone())
                .collect::<Vec<_>>(),
        )?;
        ensure!(
            self.cases_sha256 == self.cases.digest()?
                && self.contract_sha256 == self.cases.contract_digest()?,
            "Realtime report identity mismatch"
        );
        if let Some(identity) = identity {
            return self.validate_production(samples, identity);
        }
        ensure!(
            self.policy_version == POLICY,
            "historical report cannot claim production policy"
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
                    row.segments
                        .iter()
                        .all(|trace| trace.prompt_input.is_some()),
                    "Realtime trace input layout does not match its policy"
                );
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

    fn validate_production(
        &self,
        samples: &[super::regression::SampleResult],
        identity: &super::production::Identity,
    ) -> Result<()> {
        ensure!(
            self.policy_version == super::production::POLICY
                && self.cases.schema_version == 3
                && identity.cases_contract_sha256.as_deref() == Some(self.contract_sha256.as_str())
                && identity.seed_sha256
                    == super::production::digest(
                        &self
                            .cases
                            .cases
                            .iter()
                            .map(|case| (&case.id, &case.seed))
                            .collect::<Vec<_>>()
                    )?,
            "scenario execution identity mismatch"
        );
        let mut rows = self.steps.iter();
        for case in &self.cases.cases {
            let mut executions = Vec::new();
            let mut at = case.at;
            for step in &case.steps {
                at = at
                    .checked_add(step.advance_seconds)
                    .ok_or_else(|| anyhow::anyhow!("scenario clock overflow"))?;
                let row = rows
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing production step"))?;
                let sample = samples
                    .iter()
                    .find(|sample| sample.sample_id == step.sample_id)
                    .ok_or_else(|| anyhow::anyhow!("unknown production sample"))?;
                let execution = sample
                    .execution
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing production step evidence"))?;
                ensure!(
                    row.case_id == case.id
                        && row.sample_id == step.sample_id
                        && row.at == at
                        && execution.started_at == at
                        && execution.feedback == step.feedback
                        && row.before == execution.before
                        && row.after == execution.after
                        && row.segments == execution.asr
                        && row.checks == step.expect.check_production(execution),
                    "scenario feedback, clock, trace or checks differ from evidence"
                );
                executions.push(execution.clone());
                at = execution.finished_at;
            }
            super::production::validate_replay(identity, &case.seed, &executions)?;
        }
        ensure!(rows.next().is_none(), "unexpected production steps");
        Ok(())
    }
}

/// One real production pipeline per scenario; only explicit delivered feedback learns.
pub(super) fn run_production(
    cases: RealtimeCases,
    document: &crate::core::config::ConfigDocument,
    candidate: Option<String>,
    audio: &[(String, PathBuf)],
) -> Result<(RealtimeReport, super::production::Outcomes)> {
    let mut outcomes = HashMap::new();
    let mut steps = Vec::new();
    for case in &cases.cases {
        let runner =
            super::production::Runner::new(document, candidate.clone(), &case.seed, case.at)?;
        for (index, step) in case.steps.iter().enumerate() {
            let path = &audio
                .iter()
                .find(|(id, _)| id == &step.sample_id)
                .expect("validated sample coverage")
                .1;
            let result = runner.recognize(
                path,
                crate::session::SessionId(index as u64 + 1),
                step.advance_seconds,
                Some(&step.feedback),
            )?;
            let checks = step.expect.check_production(&result);
            steps.push(StepResult {
                case_id: case.id.clone(),
                sample_id: step.sample_id.clone(),
                at: result.started_at,
                before: result.before.clone(),
                segments: result.asr.clone(),
                after: result.after.clone(),
                checks,
            });
            outcomes.insert(step.sample_id.clone(), result);
        }
    }
    Ok((
        RealtimeReport {
            policy_version: super::production::POLICY.into(),
            cases_sha256: cases.digest()?,
            contract_sha256: cases.contract_digest()?,
            cases,
            steps,
        },
        outcomes,
    ))
}

#[cfg(test)]
mod tests;
