use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::regression::{
    AGENT_REVIEW_RUBRIC_VERSION, AgentDifference, AgentSampleReview, EvaluationReport, GateResults,
    LlmAggregate, RegressionError, ReviewSummary, RunStatus, unix_time_ms,
};

type Result<T> = std::result::Result<T, RegressionError>;

const REVIEW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentReviewInput {
    schema_version: u32,
    run_id: String,
    rubric_version: String,
    samples: Vec<AgentReviewSampleInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentReviewSampleInput {
    sample_id: String,
    score: u8,
    reason: String,
    differences: Vec<AgentDifference>,
}

pub(crate) fn apply_review(report_path: &Path, review_path: &Path) -> Result<EvaluationReport> {
    let mut report = EvaluationReport::read(report_path)?;
    if report.status != RunStatus::AwaitingAgentReview {
        return Err(RegressionError::Invalid(
            "agent review can only be applied to an awaiting_agent_review report".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(review_path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(RegressionError::Invalid(format!(
            "agent review must be a regular JSON file: {}",
            review_path.display()
        )));
    }
    let input: AgentReviewInput = serde_json::from_slice(&fs::read(review_path)?)?;
    validate_input_identity(&input, &report)?;

    let mut reviews = HashMap::with_capacity(input.samples.len());
    for sample in input.samples {
        validate_sample_review(&sample)?;
        let id = sample.sample_id.clone();
        if reviews
            .insert(
                id.clone(),
                AgentSampleReview {
                    score: sample.score,
                    reason: sample.reason,
                    differences: sample.differences,
                },
            )
            .is_some()
        {
            return Err(RegressionError::Invalid(format!(
                "agent review contains duplicate sample ID {id}"
            )));
        }
    }
    let expected = report
        .samples
        .iter()
        .map(|sample| sample.sample_id.as_str())
        .collect::<HashSet<_>>();
    let actual = reviews.keys().map(String::as_str).collect::<HashSet<_>>();
    if actual != expected {
        return Err(RegressionError::Invalid(
            "agent review sample coverage does not exactly match the report".to_string(),
        ));
    }

    let mut score_total = 0_u64;
    for sample in &mut report.samples {
        let review = reviews
            .remove(&sample.sample_id)
            .expect("coverage was checked before report mutation");
        score_total += u64::from(review.score);
        sample.agent_review = Some(review);
    }
    let mean_score = score_total as f64 / report.samples.len() as f64;
    let gates = GateResults {
        wer: report.aggregates.wer.wer_percent <= report.thresholds.max_wer_percent,
        llm: mean_score >= report.thresholds.min_llm_score,
        proper_nouns: report.aggregates.proper_nouns.accuracy_percent
            >= report.thresholds.min_proper_noun_percent,
    };
    let meets_targets = gates.wer && gates.llm && gates.proper_nouns;
    let reviewed_at_unix_ms = unix_time_ms()?;
    report.status = RunStatus::Complete;
    report.completed_at_unix_ms = Some(reviewed_at_unix_ms);
    report.aggregates.llm = Some(LlmAggregate {
        mean_score,
        scored_samples: report.samples.len(),
    });
    report.review = Some(ReviewSummary {
        source: "coding_agent".to_string(),
        rubric_version: AGENT_REVIEW_RUBRIC_VERSION.to_string(),
        reviewed_at_unix_ms,
    });
    report.gates = Some(gates);
    report.meets_targets = Some(meets_targets);
    if let Some(comparison) = &mut report.comparison {
        comparison.llm_score_delta = Some(mean_score - comparison.prior_llm_mean_score);
        for sample in &report.samples {
            let comparison = comparison
                .samples
                .iter_mut()
                .find(|comparison| comparison.sample_id == sample.sample_id)
                .expect("comparison contains every compatible sample");
            comparison.llm_score_delta = Some(
                f64::from(
                    sample
                        .agent_review
                        .as_ref()
                        .expect("review was just assigned")
                        .score,
                ) - f64::from(comparison.prior_llm_score),
            );
        }
    }
    report.validate_contract()?;
    report.write(report_path)?;
    Ok(report)
}

fn validate_input_identity(input: &AgentReviewInput, report: &EvaluationReport) -> Result<()> {
    if input.schema_version != REVIEW_SCHEMA_VERSION {
        return Err(RegressionError::Invalid(format!(
            "unsupported agent review schema_version {}; expected {REVIEW_SCHEMA_VERSION}",
            input.schema_version
        )));
    }
    if input.run_id != report.run_id {
        return Err(RegressionError::Invalid(format!(
            "agent review run ID {} does not match {}",
            input.run_id, report.run_id
        )));
    }
    if input.rubric_version != AGENT_REVIEW_RUBRIC_VERSION
        || input.rubric_version != report.agent_review_rubric_version
    {
        return Err(RegressionError::Invalid(
            "agent review rubric version is incompatible".to_string(),
        ));
    }
    Ok(())
}

fn validate_sample_review(sample: &AgentReviewSampleInput) -> Result<()> {
    if sample.sample_id.trim().is_empty() {
        return Err(RegressionError::Invalid(
            "agent review sample ID must not be empty".to_string(),
        ));
    }
    if sample.score > 100 {
        return Err(RegressionError::Invalid(format!(
            "agent review score for {} exceeds 100",
            sample.sample_id
        )));
    }
    if sample.reason.trim().is_empty() {
        return Err(RegressionError::Invalid(format!(
            "agent review reason for {} must not be empty",
            sample.sample_id
        )));
    }
    if sample.score < 100 && sample.differences.is_empty() {
        return Err(RegressionError::Invalid(format!(
            "agent review below 100 for {} must describe at least one difference",
            sample.sample_id
        )));
    }
    for difference in &sample.differences {
        if difference.category.trim().is_empty() || difference.explanation.trim().is_empty() {
            return Err(RegressionError::Invalid(format!(
                "agent review difference for {} needs category and explanation",
                sample.sample_id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::prompt_lab::regression::{
        AGENT_REVIEW_RUBRIC_VERSION, Aggregates, DatasetSummary, EvaluationReport,
        ProperNounAggregate, REPORT_SCHEMA_VERSION, RunStatus, SCORING_POLICY_VERSION,
        SampleResult, Thresholds, WerAggregate,
    };
    use crate::prompt_lab::{ProperNounAnnotation, SttSnapshot, score_proper_nouns, score_wer};

    fn awaiting_report() -> EvaluationReport {
        let reference = "使用 Codex".to_string();
        let hypothesis = "使用 Codex".to_string();
        EvaluationReport {
            schema_version: REPORT_SCHEMA_VERSION,
            run_id: "run-test-1".to_string(),
            created_at_unix_ms: 1,
            completed_at_unix_ms: None,
            status: RunStatus::AwaitingAgentReview,
            scoring_policy_version: SCORING_POLICY_VERSION.to_string(),
            agent_review_rubric_version: AGENT_REVIEW_RUBRIC_VERSION.to_string(),
            dataset: DatasetSummary {
                root: "/tmp/lab".to_string(),
                digest: "digest".to_string(),
                ready_sample_ids: vec!["sample-1-1".to_string()],
                pending_count: 0,
                invalid_count: 0,
            },
            stt: SttSnapshot {
                endpoint: "https://api.example.test/v1/audio/transcriptions".to_string(),
                model: "whisper-test".to_string(),
                language: Some("zh".to_string()),
                temperature: 0.0,
                prompt: Some("candidate".to_string()),
            },
            prompt_sha256: crate::prompt_lab::dataset::sha256_bytes(
                &serde_json::to_vec(&Some("candidate".to_string())).unwrap(),
            ),
            thresholds: Thresholds {
                max_wer_percent: 0.0,
                min_llm_score: 100.0,
                min_proper_noun_percent: 100.0,
            },
            aggregates: Aggregates {
                wer: WerAggregate {
                    reference_words: 2,
                    substitutions: 0,
                    deletions: 0,
                    insertions: 0,
                    wer_percent: 0.0,
                },
                proper_nouns: ProperNounAggregate {
                    matched_occurrences: 1,
                    expected_occurrences: 1,
                    accuracy_percent: 100.0,
                },
                llm: None,
            },
            review: None,
            gates: None,
            meets_targets: None,
            samples: vec![SampleResult {
                sample_id: "sample-1-1".to_string(),
                audio_path: "audio/sample-1-1.wav".to_string(),
                audio_sha256: "audio-digest".to_string(),
                reference: reference.clone(),
                proper_noun_annotations: vec![ProperNounAnnotation {
                    canonical: "Codex".to_string(),
                    accepted: Vec::new(),
                    case_sensitive: true,
                    expected_occurrences: 1,
                }],
                hypothesis: Some(hypothesis.clone()),
                wer: Some(score_wer(&reference, &hypothesis)),
                proper_nouns: Some(score_proper_nouns(
                    &hypothesis,
                    &[ProperNounAnnotation {
                        canonical: "Codex".to_string(),
                        accepted: Vec::new(),
                        case_sensitive: true,
                        expected_occurrences: 1,
                    }],
                )),
                agent_review: None,
                error: None,
            }],
            failures: Vec::new(),
            comparison: None,
        }
    }

    #[test]
    fn complete_agent_review_finalizes_report_and_all_gates() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        awaiting_report().write(&report_path).unwrap();
        fs::write(
            &review_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "run_id": "run-test-1",
                "rubric_version": AGENT_REVIEW_RUBRIC_VERSION,
                "samples": [{
                    "sample_id": "sample-1-1",
                    "score": 100,
                    "reason": "语义完全一致",
                    "differences": []
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = apply_review(&report_path, &review_path).unwrap();

        assert_eq!(report.status, RunStatus::Complete);
        assert_eq!(report.aggregates.llm.as_ref().unwrap().mean_score, 100.0);
        assert_eq!(report.meets_targets, Some(true));
        assert!(report.gates.as_ref().unwrap().wer);
        assert!(EvaluationReport::read(&report_path).is_ok());
    }

    #[test]
    fn incomplete_review_is_rejected_without_rewriting_report() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        awaiting_report().write(&report_path).unwrap();
        let before = fs::read(&report_path).unwrap();
        fs::write(
            &review_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "run_id": "run-test-1",
                "rubric_version": AGENT_REVIEW_RUBRIC_VERSION,
                "samples": []
            }))
            .unwrap(),
        )
        .unwrap();

        let error = apply_review(&report_path, &review_path).unwrap_err();

        assert!(error.to_string().contains("coverage"));
        assert_eq!(fs::read(&report_path).unwrap(), before);
    }

    #[test]
    fn out_of_range_score_is_rejected_without_rewriting_report() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        awaiting_report().write(&report_path).unwrap();
        let before = fs::read(&report_path).unwrap();
        fs::write(
            &review_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "run_id": "run-test-1",
                "rubric_version": AGENT_REVIEW_RUBRIC_VERSION,
                "samples": [{
                    "sample_id": "sample-1-1",
                    "score": 101,
                    "reason": "invalid",
                    "differences": [{
                        "category": "other",
                        "reference": null,
                        "hypothesis": null,
                        "explanation": "invalid"
                    }]
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let error = apply_review(&report_path, &review_path).unwrap_err();

        assert!(error.to_string().contains("exceeds 100"));
        assert_eq!(fs::read(&report_path).unwrap(), before);
    }
}
