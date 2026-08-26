use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::dataset::{DatasetError, sha256_bytes};
use super::{
    DatasetStore, ProperNounAnnotation, ProperNounScore, ScoringDataset, SttSnapshot, WerScore,
    score_proper_nouns, score_wer,
};
use crate::audio::WavChunkReader;
use crate::text::merge_texts;
use crate::transcriber::Transcriber;

pub(crate) const REPORT_SCHEMA_VERSION: u32 = 1;
pub(crate) const SCORING_POLICY_VERSION: &str = "stt-prompt-scoring-v2";
pub(crate) const AGENT_REVIEW_RUBRIC_VERSION: &str = "semantic-equivalence-v1";

type Result<T> = std::result::Result<T, RegressionError>;

#[derive(Debug)]
pub(crate) enum RegressionError {
    Dataset(DatasetError),
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for RegressionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dataset(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "regression I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "invalid regression JSON: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RegressionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Dataset(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<DatasetError> for RegressionError {
    fn from(error: DatasetError) -> Self {
        Self::Dataset(error)
    }
}

impl From<std::io::Error> for RegressionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RegressionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Thresholds {
    pub(crate) max_wer_percent: f64,
    pub(crate) min_llm_score: f64,
    pub(crate) min_proper_noun_percent: f64,
}

impl Thresholds {
    fn validate(&self) -> Result<()> {
        if !self.max_wer_percent.is_finite() || self.max_wer_percent < 0.0 {
            return Err(RegressionError::Invalid(
                "max WER percent must be a finite non-negative number".to_string(),
            ));
        }
        for (name, value) in [
            ("minimum LLM score", self.min_llm_score),
            ("minimum proper-noun percent", self.min_proper_noun_percent),
        ] {
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err(RegressionError::Invalid(format!(
                    "{name} must be between 0 and 100"
                )));
            }
        }
        Ok(())
    }
}

pub(crate) struct EvaluationRequest {
    pub(crate) stt: SttSnapshot,
    pub(crate) language: Option<String>,
    pub(crate) max_chunk_duration_secs: u32,
    pub(crate) max_chunk_size_bytes: u64,
    pub(crate) thresholds: Thresholds,
    pub(crate) output: Option<PathBuf>,
    pub(crate) compare_to: Option<EvaluationReport>,
}

#[derive(Debug)]
pub(crate) struct EvaluationOutcome {
    pub(crate) report: EvaluationReport,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunStatus {
    Incomplete,
    AwaitingAgentReview,
    Complete,
}

impl RunStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Incomplete => "incomplete",
            Self::AwaitingAgentReview => "awaiting_agent_review",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatasetSummary {
    pub(crate) root: String,
    pub(crate) digest: String,
    pub(crate) ready_sample_ids: Vec<String>,
    pub(crate) pending_count: usize,
    pub(crate) invalid_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WerAggregate {
    pub(crate) reference_words: u64,
    pub(crate) substitutions: u64,
    pub(crate) deletions: u64,
    pub(crate) insertions: u64,
    pub(crate) wer_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProperNounAggregate {
    pub(crate) matched_occurrences: u64,
    pub(crate) expected_occurrences: u64,
    pub(crate) accuracy_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LlmAggregate {
    pub(crate) mean_score: f64,
    pub(crate) scored_samples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Aggregates {
    pub(crate) wer: WerAggregate,
    pub(crate) proper_nouns: ProperNounAggregate,
    pub(crate) llm: Option<LlmAggregate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentDifference {
    pub(crate) category: String,
    pub(crate) reference: Option<String>,
    pub(crate) hypothesis: Option<String>,
    pub(crate) explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentSampleReview {
    pub(crate) score: u8,
    pub(crate) reason: String,
    pub(crate) differences: Vec<AgentDifference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewSummary {
    pub(crate) source: String,
    pub(crate) rubric_version: String,
    pub(crate) reviewed_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateResults {
    pub(crate) wer: bool,
    pub(crate) llm: bool,
    pub(crate) proper_nouns: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SampleResult {
    pub(crate) sample_id: String,
    pub(crate) audio_path: String,
    pub(crate) audio_sha256: String,
    pub(crate) reference: String,
    pub(crate) proper_noun_annotations: Vec<ProperNounAnnotation>,
    pub(crate) hypothesis: Option<String>,
    pub(crate) wer: Option<WerScore>,
    pub(crate) proper_nouns: Option<ProperNounScore>,
    pub(crate) agent_review: Option<AgentSampleReview>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunFailure {
    pub(crate) sample_id: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SampleComparison {
    pub(crate) sample_id: String,
    pub(crate) wer_percent_delta: Option<f64>,
    pub(crate) proper_noun_percent_delta: Option<f64>,
    pub(crate) llm_score_delta: Option<f64>,
    pub(crate) prior_llm_score: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunComparison {
    pub(crate) prior_run_id: String,
    pub(crate) wer_percent_delta: f64,
    pub(crate) proper_noun_percent_delta: f64,
    pub(crate) llm_score_delta: Option<f64>,
    pub(crate) prior_llm_mean_score: f64,
    pub(crate) samples: Vec<SampleComparison>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvaluationReport {
    pub(crate) schema_version: u32,
    pub(crate) run_id: String,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) completed_at_unix_ms: Option<u64>,
    pub(crate) status: RunStatus,
    pub(crate) scoring_policy_version: String,
    pub(crate) agent_review_rubric_version: String,
    pub(crate) dataset: DatasetSummary,
    pub(crate) stt: SttSnapshot,
    pub(crate) prompt_sha256: String,
    pub(crate) thresholds: Thresholds,
    pub(crate) aggregates: Aggregates,
    pub(crate) review: Option<ReviewSummary>,
    pub(crate) gates: Option<GateResults>,
    pub(crate) meets_targets: Option<bool>,
    pub(crate) samples: Vec<SampleResult>,
    pub(crate) failures: Vec<RunFailure>,
    pub(crate) comparison: Option<RunComparison>,
}

impl EvaluationReport {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(RegressionError::Invalid(format!(
                "report must be a regular JSON file: {}",
                path.display()
            )));
        }
        let report: Self = serde_json::from_slice(&fs::read(path)?)?;
        report.validate_contract()?;
        Ok(report)
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        fs::write(path, bytes)?;
        Ok(())
    }

    pub(crate) fn validate_contract(&self) -> Result<()> {
        if self.schema_version != REPORT_SCHEMA_VERSION {
            return Err(RegressionError::Invalid(format!(
                "unsupported report schema_version {}; expected {REPORT_SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        if self.scoring_policy_version != SCORING_POLICY_VERSION
            || self.agent_review_rubric_version != AGENT_REVIEW_RUBRIC_VERSION
        {
            return Err(RegressionError::Invalid(
                "report policy or rubric version is incompatible".to_string(),
            ));
        }
        self.thresholds.validate()?;
        validate_stt(&self.stt)?;
        let expected_prompt_digest = sha256_bytes(&serde_json::to_vec(&self.stt.prompt)?);
        if self.prompt_sha256 != expected_prompt_digest {
            return Err(RegressionError::Invalid(
                "report prompt digest does not match its STT prompt".to_string(),
            ));
        }
        let sample_ids = self
            .samples
            .iter()
            .map(|sample| sample.sample_id.as_str())
            .collect::<Vec<_>>();
        if sample_ids.is_empty() {
            return Err(RegressionError::Invalid(
                "report contains no scored samples".to_string(),
            ));
        }
        let unique_ids = sample_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let ready_ids = self
            .dataset
            .ready_sample_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if sample_ids != ready_ids || unique_ids.len() != sample_ids.len() {
            return Err(RegressionError::Invalid(
                "report sample coverage does not match the dataset summary".to_string(),
            ));
        }
        let local_aggregates = aggregate_local_scores(&self.samples);
        if !same_wer_aggregate(&self.aggregates.wer, &local_aggregates.wer)
            || !same_proper_noun_aggregate(
                &self.aggregates.proper_nouns,
                &local_aggregates.proper_nouns,
            )
        {
            return Err(RegressionError::Invalid(
                "report local aggregates do not match its per-sample metrics".to_string(),
            ));
        }
        let sample_failures = self
            .samples
            .iter()
            .filter(|sample| sample.error.is_some())
            .count();
        if sample_failures != self.failures.len()
            || self.failures.iter().any(|failure| {
                !self.samples.iter().any(|sample| {
                    sample.sample_id == failure.sample_id
                        && sample.error.as_deref() == Some(failure.message.as_str())
                })
            })
        {
            return Err(RegressionError::Invalid(
                "report failure summary does not match its sample errors".to_string(),
            ));
        }
        for sample in &self.samples {
            match sample.hypothesis.as_deref() {
                Some(hypothesis) => {
                    let expected_wer = score_wer(&sample.reference, hypothesis);
                    let expected_proper_nouns =
                        score_proper_nouns(hypothesis, &sample.proper_noun_annotations);
                    if !sample
                        .wer
                        .as_ref()
                        .is_some_and(|actual| same_wer_score(actual, &expected_wer))
                        || !sample.proper_nouns.as_ref().is_some_and(|actual| {
                            same_proper_noun_score(actual, &expected_proper_nouns)
                        })
                    {
                        return Err(RegressionError::Invalid(format!(
                            "sample {} metrics do not match its reference and hypothesis",
                            sample.sample_id
                        )));
                    }
                }
                None if sample.wer.is_some() || sample.proper_nouns.is_some() => {
                    return Err(RegressionError::Invalid(format!(
                        "failed sample {} contains local metrics",
                        sample.sample_id
                    )));
                }
                None => {}
            }
        }
        match self.status {
            RunStatus::Incomplete
                if self.failures.is_empty()
                    || self.review.is_some()
                    || self.aggregates.llm.is_some()
                    || self.meets_targets.is_some()
                    || self.gates.is_some()
                    || self.completed_at_unix_ms.is_some()
                    || self
                        .samples
                        .iter()
                        .any(|sample| sample.agent_review.is_some()) =>
            {
                Err(RegressionError::Invalid(
                    "incomplete report contains inconsistent final fields".to_string(),
                ))
            }
            RunStatus::AwaitingAgentReview
                if !self.failures.is_empty()
                    || self.review.is_some()
                    || self.aggregates.llm.is_some()
                    || self.meets_targets.is_some()
                    || self.gates.is_some()
                    || self.completed_at_unix_ms.is_some()
                    || self.samples.iter().any(|sample| {
                        sample.agent_review.is_some()
                            || sample.hypothesis.is_none()
                            || sample.wer.is_none()
                            || sample.proper_nouns.is_none()
                            || sample.error.is_some()
                    }) =>
            {
                Err(RegressionError::Invalid(
                    "awaiting-review report already contains final review fields".to_string(),
                ))
            }
            RunStatus::Complete
                if self.review.is_none()
                    || self.aggregates.llm.is_none()
                    || self.meets_targets.is_none()
                    || self.gates.is_none()
                    || self.completed_at_unix_ms.is_none()
                    || !self.failures.is_empty()
                    || self.samples.iter().any(|sample| {
                        sample.agent_review.is_none()
                            || sample.hypothesis.is_none()
                            || sample.wer.is_none()
                            || sample.proper_nouns.is_none()
                            || sample.error.is_some()
                    }) =>
            {
                Err(RegressionError::Invalid(
                    "complete report is missing final review fields".to_string(),
                ))
            }
            RunStatus::Complete => self.validate_complete_review(),
            _ => Ok(()),
        }?;
        self.validate_comparison_contract()
    }

    fn validate_complete_review(&self) -> Result<()> {
        let review = self.review.as_ref().expect("complete fields were checked");
        if review.source != "coding_agent"
            || review.rubric_version != AGENT_REVIEW_RUBRIC_VERSION
            || review.reviewed_at_unix_ms != self.completed_at_unix_ms.unwrap()
        {
            return Err(RegressionError::Invalid(
                "complete report review metadata is invalid".to_string(),
            ));
        }
        let mut total = 0_u64;
        for sample in &self.samples {
            let review = sample
                .agent_review
                .as_ref()
                .expect("complete sample review was checked");
            if review.score > 100 || review.reason.trim().is_empty() {
                return Err(RegressionError::Invalid(format!(
                    "sample {} has an invalid agent review",
                    sample.sample_id
                )));
            }
            if review.score < 100 && review.differences.is_empty() {
                return Err(RegressionError::Invalid(format!(
                    "sample {} review below 100 has no differences",
                    sample.sample_id
                )));
            }
            if review.differences.iter().any(|difference| {
                difference.category.trim().is_empty() || difference.explanation.trim().is_empty()
            }) {
                return Err(RegressionError::Invalid(format!(
                    "sample {} has an invalid structured difference",
                    sample.sample_id
                )));
            }
            total += u64::from(review.score);
        }
        let llm = self
            .aggregates
            .llm
            .as_ref()
            .expect("complete LLM aggregate was checked");
        let mean = total as f64 / self.samples.len() as f64;
        if llm.scored_samples != self.samples.len() || !float_eq(llm.mean_score, mean) {
            return Err(RegressionError::Invalid(
                "LLM aggregate does not match per-sample reviews".to_string(),
            ));
        }
        let expected_gates = GateResults {
            wer: self.aggregates.wer.wer_percent <= self.thresholds.max_wer_percent,
            llm: llm.mean_score >= self.thresholds.min_llm_score,
            proper_nouns: self.aggregates.proper_nouns.accuracy_percent
                >= self.thresholds.min_proper_noun_percent,
        };
        let gates = self.gates.as_ref().expect("complete gates were checked");
        let meets = gates.wer && gates.llm && gates.proper_nouns;
        if gates.wer != expected_gates.wer
            || gates.llm != expected_gates.llm
            || gates.proper_nouns != expected_gates.proper_nouns
            || self.meets_targets != Some(meets)
        {
            return Err(RegressionError::Invalid(
                "complete report gates do not match metrics and thresholds".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_comparison_contract(&self) -> Result<()> {
        let Some(comparison) = &self.comparison else {
            return Ok(());
        };
        let ids = comparison
            .samples
            .iter()
            .map(|sample| sample.sample_id.as_str())
            .collect::<Vec<_>>();
        let report_ids = self
            .samples
            .iter()
            .map(|sample| sample.sample_id.as_str())
            .collect::<Vec<_>>();
        if comparison.prior_run_id.trim().is_empty()
            || ids != report_ids
            || !(0.0..=100.0).contains(&comparison.prior_llm_mean_score)
            || !comparison.wer_percent_delta.is_finite()
            || !comparison.proper_noun_percent_delta.is_finite()
            || comparison.samples.iter().any(|sample| {
                sample.prior_llm_score > 100
                    || sample
                        .wer_percent_delta
                        .is_some_and(|delta| !delta.is_finite())
                    || sample
                        .proper_noun_percent_delta
                        .is_some_and(|delta| !delta.is_finite())
            })
        {
            return Err(RegressionError::Invalid(
                "report comparison metadata is invalid".to_string(),
            ));
        }
        match self.status {
            RunStatus::Complete => {
                let mean = self
                    .aggregates
                    .llm
                    .as_ref()
                    .expect("complete report fields were checked")
                    .mean_score;
                if !comparison
                    .llm_score_delta
                    .is_some_and(|delta| float_eq(delta, mean - comparison.prior_llm_mean_score))
                    || comparison.samples.iter().any(|comparison| {
                        let score = self
                            .samples
                            .iter()
                            .find(|sample| sample.sample_id == comparison.sample_id)
                            .and_then(|sample| sample.agent_review.as_ref())
                            .map(|review| f64::from(review.score));
                        !comparison.llm_score_delta.is_some_and(|delta| {
                            score.is_some_and(|score| {
                                float_eq(delta, score - f64::from(comparison.prior_llm_score))
                            })
                        })
                    })
                {
                    return Err(RegressionError::Invalid(
                        "report comparison LLM deltas are invalid".to_string(),
                    ));
                }
            }
            RunStatus::Incomplete | RunStatus::AwaitingAgentReview => {
                if comparison.llm_score_delta.is_some()
                    || comparison
                        .samples
                        .iter()
                        .any(|sample| sample.llm_score_delta.is_some())
                {
                    return Err(RegressionError::Invalid(
                        "unfinished report comparison contains LLM deltas".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn same_wer_aggregate(left: &WerAggregate, right: &WerAggregate) -> bool {
    left.reference_words == right.reference_words
        && left.substitutions == right.substitutions
        && left.deletions == right.deletions
        && left.insertions == right.insertions
        && float_eq(left.wer_percent, right.wer_percent)
}

fn same_wer_score(left: &WerScore, right: &WerScore) -> bool {
    left.reference_words == right.reference_words
        && left.substitutions == right.substitutions
        && left.deletions == right.deletions
        && left.insertions == right.insertions
        && float_eq(left.wer_percent, right.wer_percent)
        && left.alignment == right.alignment
}

fn same_proper_noun_aggregate(left: &ProperNounAggregate, right: &ProperNounAggregate) -> bool {
    left.matched_occurrences == right.matched_occurrences
        && left.expected_occurrences == right.expected_occurrences
        && float_eq(left.accuracy_percent, right.accuracy_percent)
}

fn same_proper_noun_score(left: &ProperNounScore, right: &ProperNounScore) -> bool {
    left.matched_occurrences == right.matched_occurrences
        && left.expected_occurrences == right.expected_occurrences
        && float_eq(left.accuracy_percent, right.accuracy_percent)
        && left.annotations == right.annotations
}

fn float_eq(left: f64, right: f64) -> bool {
    (left - right).abs() <= f64::EPSILON * left.abs().max(right.abs()).max(1.0) * 8.0
}

pub(crate) fn evaluate(
    store: &DatasetStore,
    transcriber: &dyn Transcriber,
    request: EvaluationRequest,
) -> Result<EvaluationOutcome> {
    request.thresholds.validate()?;
    validate_stt(&request.stt)?;
    let dataset = store.scoring_snapshot(SCORING_POLICY_VERSION)?;
    validate_scoring_dataset(&dataset)?;
    if let Some(prior) = request.compare_to.as_ref() {
        validate_comparison(prior, &dataset, &request.stt)?;
    }

    let created_at_unix_ms = unix_time_ms()?;
    let prompt_sha256 = sha256_bytes(&serde_json::to_vec(&request.stt.prompt)?);
    let run_id = next_run_id(created_at_unix_ms);
    let path = request
        .output
        .clone()
        .unwrap_or_else(|| store.root().join("runs").join(format!("{run_id}.json")));
    validate_new_output_path(&path)?;

    let mut samples = Vec::with_capacity(dataset.samples.len());
    let mut failures = Vec::new();
    for sample in &dataset.samples {
        let reference = sample
            .reference
            .text
            .as_deref()
            .expect("scoring snapshots contain ready references")
            .to_string();
        let result = transcribe_sample(
            &store.root().join(&sample.audio.path),
            transcriber,
            request.max_chunk_duration_secs,
            request.max_chunk_size_bytes,
            request.language.as_deref(),
        );
        match result {
            Ok(hypothesis) => samples.push(SampleResult {
                sample_id: sample.id.clone(),
                audio_path: sample.audio.path.clone(),
                audio_sha256: sample.audio.sha256.clone(),
                proper_noun_annotations: sample.reference.proper_nouns.clone(),
                wer: Some(score_wer(&reference, &hypothesis)),
                proper_nouns: Some(score_proper_nouns(
                    &hypothesis,
                    &sample.reference.proper_nouns,
                )),
                reference,
                hypothesis: Some(hypothesis),
                agent_review: None,
                error: None,
            }),
            Err(message) => {
                failures.push(RunFailure {
                    sample_id: sample.id.clone(),
                    message: message.clone(),
                });
                samples.push(SampleResult {
                    sample_id: sample.id.clone(),
                    audio_path: sample.audio.path.clone(),
                    audio_sha256: sample.audio.sha256.clone(),
                    proper_noun_annotations: sample.reference.proper_nouns.clone(),
                    reference,
                    hypothesis: None,
                    wer: None,
                    proper_nouns: None,
                    agent_review: None,
                    error: Some(message),
                });
            }
        }
    }
    let aggregates = aggregate_local_scores(&samples);
    let status = if failures.is_empty() {
        RunStatus::AwaitingAgentReview
    } else {
        RunStatus::Incomplete
    };
    let comparison = request
        .compare_to
        .as_ref()
        .map(|prior| build_comparison(prior, &samples, &aggregates));
    let report = EvaluationReport {
        schema_version: REPORT_SCHEMA_VERSION,
        run_id,
        created_at_unix_ms,
        completed_at_unix_ms: None,
        status,
        scoring_policy_version: SCORING_POLICY_VERSION.to_string(),
        agent_review_rubric_version: AGENT_REVIEW_RUBRIC_VERSION.to_string(),
        dataset: DatasetSummary {
            root: store.root().display().to_string(),
            digest: dataset.digest,
            ready_sample_ids: dataset
                .samples
                .iter()
                .map(|sample| sample.id.clone())
                .collect(),
            pending_count: dataset.pending_count,
            invalid_count: dataset.invalid_count,
        },
        stt: request.stt,
        prompt_sha256,
        thresholds: request.thresholds,
        aggregates,
        review: None,
        gates: None,
        meets_targets: None,
        samples,
        failures,
        comparison,
    };
    report.validate_contract()?;
    report.write(&path)?;
    Ok(EvaluationOutcome { report, path })
}

fn validate_new_output_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(RegressionError::Invalid(format!(
                "report output already exists: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if !fs::metadata(parent)?.is_dir() {
        return Err(RegressionError::Invalid(format!(
            "report output parent is not a directory: {}",
            parent.display()
        )));
    }
    Ok(())
}

fn validate_stt(stt: &SttSnapshot) -> Result<()> {
    if stt.endpoint.trim().is_empty() || stt.model.trim().is_empty() || !stt.temperature.is_finite()
    {
        return Err(RegressionError::Invalid(
            "STT metadata is incomplete or invalid".to_string(),
        ));
    }
    let endpoint = reqwest::Url::parse(&stt.endpoint)
        .map_err(|error| RegressionError::Invalid(format!("invalid STT endpoint: {error}")))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(RegressionError::Invalid(
            "STT endpoint metadata must be sanitized".to_string(),
        ));
    }
    Ok(())
}

fn validate_scoring_dataset(dataset: &ScoringDataset) -> Result<()> {
    if dataset.samples.is_empty() {
        return Err(RegressionError::Invalid(
            "dataset has no ready samples".to_string(),
        ));
    }
    let mut expected_proper_nouns = 0_u64;
    for sample in &dataset.samples {
        let reference = sample
            .reference
            .text
            .as_deref()
            .expect("scoring snapshots contain ready references");
        if score_wer(reference, reference).reference_words == 0 {
            return Err(RegressionError::Invalid(format!(
                "ready sample {} has no scoreable reference words",
                sample.id
            )));
        }
        expected_proper_nouns += sample
            .reference
            .proper_nouns
            .iter()
            .map(|annotation| u64::from(annotation.expected_occurrences))
            .sum::<u64>();
    }
    if expected_proper_nouns == 0 {
        return Err(RegressionError::Invalid(
            "ready dataset has no expected proper-noun occurrences".to_string(),
        ));
    }
    Ok(())
}

fn transcribe_sample(
    path: &Path,
    transcriber: &dyn Transcriber,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
    language: Option<&str>,
) -> std::result::Result<String, String> {
    let mut reader = WavChunkReader::open(path, max_chunk_duration_secs, max_chunk_size_bytes)
        .map_err(|error| error.to_string())?;
    let mut texts = Vec::new();
    for chunk in reader.chunks() {
        let chunk = chunk.map_err(|error| error.to_string())?;
        texts.push(
            transcriber
                .transcribe(&chunk)
                .map_err(|error| error.to_string())?,
        );
    }
    if texts.is_empty() {
        return Err("WAV produced no transcribable chunks".to_string());
    }
    let text = merge_texts(&texts, language);
    if text.is_empty() {
        Err("STT returned empty text".to_string())
    } else {
        Ok(text)
    }
}

fn aggregate_local_scores(samples: &[SampleResult]) -> Aggregates {
    let mut wer = WerAggregate {
        reference_words: 0,
        substitutions: 0,
        deletions: 0,
        insertions: 0,
        wer_percent: 0.0,
    };
    let mut proper_nouns = ProperNounAggregate {
        matched_occurrences: 0,
        expected_occurrences: 0,
        accuracy_percent: 0.0,
    };
    for sample in samples {
        if let Some(score) = &sample.wer {
            wer.reference_words += score.reference_words;
            wer.substitutions += score.substitutions;
            wer.deletions += score.deletions;
            wer.insertions += score.insertions;
        }
        if let Some(score) = &sample.proper_nouns {
            proper_nouns.matched_occurrences += score.matched_occurrences;
            proper_nouns.expected_occurrences += score.expected_occurrences;
        }
    }
    if wer.reference_words > 0 {
        wer.wer_percent = (wer.substitutions + wer.deletions + wer.insertions) as f64 * 100.0
            / wer.reference_words as f64;
    }
    if proper_nouns.expected_occurrences > 0 {
        proper_nouns.accuracy_percent = proper_nouns.matched_occurrences as f64 * 100.0
            / proper_nouns.expected_occurrences as f64;
    }
    Aggregates {
        wer,
        proper_nouns,
        llm: None,
    }
}

fn validate_comparison(
    prior: &EvaluationReport,
    dataset: &ScoringDataset,
    stt: &SttSnapshot,
) -> Result<()> {
    prior.validate_contract()?;
    let sample_ids = dataset
        .samples
        .iter()
        .map(|sample| sample.id.as_str())
        .collect::<Vec<_>>();
    let prior_ids = prior
        .dataset
        .ready_sample_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let compatible = prior.status == RunStatus::Complete
        && prior.dataset.digest == dataset.digest
        && prior_ids == sample_ids
        && prior.stt.endpoint == stt.endpoint
        && prior.stt.model == stt.model
        && prior.stt.language == stt.language
        && prior.stt.temperature == stt.temperature
        && prior.failures.is_empty();
    if compatible {
        Ok(())
    } else {
        Err(RegressionError::Invalid(format!(
            "comparison report {} is not compatible with this evaluation",
            prior.run_id
        )))
    }
}

fn build_comparison(
    prior: &EvaluationReport,
    samples: &[SampleResult],
    aggregates: &Aggregates,
) -> RunComparison {
    let samples = samples
        .iter()
        .map(|sample| {
            let old = prior
                .samples
                .iter()
                .find(|old| old.sample_id == sample.sample_id)
                .expect("comparison compatibility checks exact sample coverage");
            SampleComparison {
                sample_id: sample.sample_id.clone(),
                wer_percent_delta: sample
                    .wer
                    .as_ref()
                    .zip(old.wer.as_ref())
                    .map(|(new, old)| new.wer_percent - old.wer_percent),
                proper_noun_percent_delta: sample
                    .proper_nouns
                    .as_ref()
                    .zip(old.proper_nouns.as_ref())
                    .map(|(new, old)| new.accuracy_percent - old.accuracy_percent),
                llm_score_delta: None,
                prior_llm_score: old
                    .agent_review
                    .as_ref()
                    .expect("complete comparison report has per-sample reviews")
                    .score,
            }
        })
        .collect();
    RunComparison {
        prior_run_id: prior.run_id.clone(),
        wer_percent_delta: aggregates.wer.wer_percent - prior.aggregates.wer.wer_percent,
        proper_noun_percent_delta: aggregates.proper_nouns.accuracy_percent
            - prior.aggregates.proper_nouns.accuracy_percent,
        llm_score_delta: None,
        prior_llm_mean_score: prior
            .aggregates
            .llm
            .as_ref()
            .expect("complete comparison report has LLM aggregate")
            .mean_score,
        samples,
    }
}

fn next_run_id(created_at_unix_ms: u64) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    format!(
        "run-{created_at_unix_ms}-{}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn unix_time_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RegressionError::Invalid(format!("system clock failed: {error}")))?
        .as_millis()
        .try_into()
        .map_err(|_| RegressionError::Invalid("current timestamp does not fit u64".to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;

    use hound::{SampleFormat, WavSpec, WavWriter};
    use tempfile::tempdir;

    use super::*;
    use crate::prompt_lab::apply_review;
    use crate::prompt_lab::{CaptureTranscription, ProperNounAnnotation};
    use crate::transcriber::{TranscribeError, Transcriber};

    struct SequenceTranscriber(Mutex<VecDeque<std::result::Result<String, TranscribeError>>>);

    impl SequenceTranscriber {
        fn new(
            results: impl IntoIterator<Item = std::result::Result<String, TranscribeError>>,
        ) -> Self {
            Self(Mutex::new(results.into_iter().collect()))
        }
    }

    impl Transcriber for SequenceTranscriber {
        fn transcribe(
            &self,
            _chunk: &crate::audio::WavChunk,
        ) -> std::result::Result<String, TranscribeError> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .expect("one fake result per sample")
        }
    }

    fn write_wav(path: &Path) {
        let mut writer = WavWriter::create(
            path,
            WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [1_i16, 2, 3, 4] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn snapshot(prompt: Option<&str>) -> SttSnapshot {
        SttSnapshot {
            endpoint: "https://api.example.test/v1/audio/transcriptions".to_string(),
            model: "whisper-test".to_string(),
            language: Some("zh".to_string()),
            temperature: 0.0,
            prompt: prompt.map(str::to_string),
        }
    }

    fn add_ready_sample(store: &DatasetStore, timestamp: u64, reference: &str, term: &str) {
        let reservation = store.reserve_sample(timestamp).unwrap();
        write_wav(&reservation.audio_path);
        let id = reservation.id.clone();
        store
            .complete_capture(
                reservation,
                CaptureTranscription::success("initial", snapshot(Some("baseline"))),
            )
            .unwrap();
        store
            .correct_sample(
                &id,
                reference,
                vec![ProperNounAnnotation {
                    canonical: term.to_string(),
                    accepted: Vec::new(),
                    case_sensitive: true,
                    expected_occurrences: 1,
                }],
            )
            .unwrap();
    }

    fn request(output: std::path::PathBuf) -> EvaluationRequest {
        EvaluationRequest {
            stt: snapshot(Some("candidate")),
            language: Some("zh".to_string()),
            max_chunk_duration_secs: 30,
            max_chunk_size_bytes: 23 * 1024 * 1024,
            thresholds: Thresholds {
                max_wer_percent: 20.0,
                min_llm_score: 90.0,
                min_proper_noun_percent: 100.0,
            },
            output: Some(output),
            compare_to: None,
        }
    }

    #[test]
    fn evaluates_every_ready_sample_and_writes_awaiting_review_report() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        add_ready_sample(&store, 1, "使用 Codex", "Codex");
        add_ready_sample(&store, 2, "运行 ViberWhisper", "ViberWhisper");
        let output = directory.path().join("run.json");
        let transcriber = SequenceTranscriber::new([
            Ok("使用 Codex".to_string()),
            Ok("运行 ViberWhisper".to_string()),
        ]);

        let outcome = evaluate(&store, &transcriber, request(output.clone())).unwrap();

        assert_eq!(outcome.report.status, RunStatus::AwaitingAgentReview);
        assert_eq!(outcome.report.samples.len(), 2);
        assert_eq!(outcome.report.aggregates.wer.wer_percent, 0.0);
        assert_eq!(
            outcome.report.aggregates.proper_nouns.accuracy_percent,
            100.0
        );
        assert!(outcome.report.aggregates.llm.is_none());
        assert!(outcome.report.meets_targets.is_none());
        assert_eq!(outcome.path, output);
        assert_eq!(
            EvaluationReport::read(&outcome.path).unwrap().run_id,
            outcome.report.run_id
        );
    }

    #[test]
    fn sample_failure_is_reported_without_skipping_later_samples() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        add_ready_sample(&store, 1, "使用 Codex", "Codex");
        add_ready_sample(&store, 2, "运行 ViberWhisper", "ViberWhisper");
        let transcriber = SequenceTranscriber::new([
            Err(TranscribeError::Network("offline".to_string())),
            Ok("运行 ViberWhisper".to_string()),
        ]);

        let outcome = evaluate(
            &store,
            &transcriber,
            request(directory.path().join("failed.json")),
        )
        .unwrap();

        assert_eq!(outcome.report.status, RunStatus::Incomplete);
        assert_eq!(outcome.report.failures.len(), 1);
        assert!(outcome.report.samples[0].error.is_some());
        assert!(outcome.report.samples[1].hypothesis.is_some());
    }

    #[test]
    fn dataset_wer_is_a_micro_average_of_edit_totals() {
        fn sample(id: &str, reference: &str, hypothesis: &str) -> SampleResult {
            SampleResult {
                sample_id: id.to_string(),
                audio_path: format!("audio/{id}.wav"),
                audio_sha256: "digest".to_string(),
                reference: reference.to_string(),
                proper_noun_annotations: Vec::new(),
                hypothesis: Some(hypothesis.to_string()),
                wer: Some(score_wer(reference, hypothesis)),
                proper_nouns: Some(score_proper_nouns(hypothesis, &[])),
                agent_review: None,
                error: None,
            }
        }
        let samples = [
            sample("sample-1-1", "wrong", "changed"),
            sample(
                "sample-2-1",
                "one two three four five six seven eight nine",
                "one two three four five six seven eight nine",
            ),
        ];

        let aggregates = aggregate_local_scores(&samples);

        assert_eq!(aggregates.wer.reference_words, 10);
        assert_eq!(aggregates.wer.substitutions, 1);
        assert_eq!(aggregates.wer.wer_percent, 10.0);
    }

    #[test]
    fn compatible_complete_run_produces_local_and_reviewed_deltas() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        add_ready_sample(&store, 1, "使用 Codex", "Codex");

        let baseline_path = directory.path().join("baseline.json");
        let baseline = evaluate(
            &store,
            &SequenceTranscriber::new([Ok("使用 Codex".to_string())]),
            request(baseline_path.clone()),
        )
        .unwrap();
        let baseline_review = directory.path().join("baseline-review.json");
        write_review(&baseline_review, &baseline.report.run_id, 100, false);
        let baseline = apply_review(&baseline_path, &baseline_review).unwrap();

        let candidate_path = directory.path().join("candidate.json");
        let mut candidate_request = request(candidate_path.clone());
        candidate_request.compare_to = Some(baseline);
        let candidate = evaluate(
            &store,
            &SequenceTranscriber::new([Ok("使用 Code".to_string())]),
            candidate_request,
        )
        .unwrap();

        let comparison = candidate.report.comparison.as_ref().unwrap();
        assert!(comparison.wer_percent_delta > 0.0);
        assert_eq!(comparison.proper_noun_percent_delta, -100.0);
        assert!(comparison.llm_score_delta.is_none());

        let candidate_review = directory.path().join("candidate-review.json");
        write_review(&candidate_review, &candidate.report.run_id, 80, true);
        let candidate = apply_review(&candidate_path, &candidate_review).unwrap();
        let comparison = candidate.comparison.unwrap();
        assert_eq!(comparison.llm_score_delta, Some(-20.0));
        assert_eq!(comparison.samples[0].llm_score_delta, Some(-20.0));
    }

    #[test]
    fn incompatible_comparison_is_rejected_before_transcription() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        add_ready_sample(&store, 1, "使用 Codex", "Codex");
        let baseline_path = directory.path().join("baseline.json");
        let baseline = evaluate(
            &store,
            &SequenceTranscriber::new([Ok("使用 Codex".to_string())]),
            request(baseline_path.clone()),
        )
        .unwrap();
        let review_path = directory.path().join("review.json");
        write_review(&review_path, &baseline.report.run_id, 100, false);
        let mut baseline = apply_review(&baseline_path, &review_path).unwrap();
        baseline.stt.model = "another-model".to_string();
        let mut candidate_request = request(directory.path().join("candidate.json"));
        candidate_request.compare_to = Some(baseline);

        let error = evaluate(&store, &SequenceTranscriber::new([]), candidate_request).unwrap_err();

        assert!(error.to_string().contains("not compatible"));
    }

    #[test]
    fn v1_comparison_is_rejected_before_v2_transcription() {
        assert_eq!(SCORING_POLICY_VERSION, "stt-prompt-scoring-v2");
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        add_ready_sample(&store, 1, "使用 Codex", "Codex");
        let baseline_path = directory.path().join("baseline.json");
        let baseline = evaluate(
            &store,
            &SequenceTranscriber::new([Ok("使用 Codex".to_string())]),
            request(baseline_path.clone()),
        )
        .unwrap();
        let review_path = directory.path().join("review.json");
        write_review(&review_path, &baseline.report.run_id, 100, false);
        let mut baseline = apply_review(&baseline_path, &review_path).unwrap();
        baseline.scoring_policy_version = "stt-prompt-scoring-v1".to_string();
        let mut candidate_request = request(directory.path().join("candidate.json"));
        candidate_request.compare_to = Some(baseline);

        let error = evaluate(&store, &SequenceTranscriber::new([]), candidate_request).unwrap_err();

        assert!(error.to_string().contains("policy or rubric version"));
    }

    #[test]
    fn dataset_without_proper_noun_denominator_is_rejected_before_transcription() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        let reservation = store.reserve_sample(1).unwrap();
        write_wav(&reservation.audio_path);
        let id = reservation.id.clone();
        store
            .complete_capture(
                reservation,
                CaptureTranscription::success("initial", snapshot(Some("baseline"))),
            )
            .unwrap();
        store
            .correct_sample(&id, "普通参考文本", Vec::new())
            .unwrap();

        let error = evaluate(
            &store,
            &SequenceTranscriber::new([]),
            request(directory.path().join("run.json")),
        )
        .unwrap_err();

        assert!(error.to_string().contains("proper-noun"));
    }

    fn write_review(path: &Path, run_id: &str, score: u8, with_difference: bool) {
        let differences = if with_difference {
            vec![serde_json::json!({
                "category": "proper_noun",
                "reference": "Codex",
                "hypothesis": "Code",
                "explanation": "专有名词识别错误"
            })]
        } else {
            Vec::new()
        };
        fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "run_id": run_id,
                "rubric_version": AGENT_REVIEW_RUBRIC_VERSION,
                "samples": [{
                    "sample_id": "sample-1-1",
                    "score": score,
                    "reason": if score == 100 { "语义一致" } else { "专有名词错误" },
                    "differences": differences
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
