use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::regression::{
    AGENT_REVIEW_RUBRIC_VERSION, AgentDifference, AgentSampleReview, EvaluationReport, GateResults,
    RegressionError, ReviewSummary, RunStatus, unix_time_ms,
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
        let id = sample.sample_id;
        if id.trim().is_empty() {
            return Err(RegressionError::Invalid(
                "agent review sample ID must not be empty".to_string(),
            ));
        }
        let review = AgentSampleReview {
            score: sample.score,
            reason: sample.reason,
            differences: sample.differences,
        };
        review.validate(&id)?;
        if reviews.insert(id.clone(), review).is_some() {
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

    for sample in &mut report.samples {
        let review = reviews
            .remove(&sample.sample_id)
            .expect("coverage was checked before report mutation");
        sample.agent_review = Some(review);
    }
    let llm = report.review_aggregate();
    let mean_score = llm.mean_score;
    let gates = GateResults::evaluate(&report.aggregates, mean_score, &report.thresholds);
    let meets_targets = gates.all_passed();
    let reviewed_at_unix_ms = unix_time_ms()?;
    report.status = RunStatus::Complete;
    report.completed_at_unix_ms = Some(reviewed_at_unix_ms);
    report.aggregates.llm = Some(llm);
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::{Value, json};
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

    fn write_perfect_review(path: &Path) {
        fs::write(
            path,
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
    }

    #[test]
    fn complete_agent_review_aggregates_samples_and_round_trips() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        let mut report = awaiting_report();
        let mut second_sample = report.samples[0].clone();
        second_sample.sample_id = "sample-1-2".to_string();
        second_sample.audio_path = "audio/sample-1-2.wav".to_string();
        report
            .dataset
            .ready_sample_ids
            .push(second_sample.sample_id.clone());
        report.samples.push(second_sample);
        report.aggregates.wer.reference_words = 4;
        report.aggregates.proper_nouns.matched_occurrences = 2;
        report.aggregates.proper_nouns.expected_occurrences = 2;
        report.thresholds.min_llm_score = 99.5;
        report.write(&report_path).unwrap();
        write_perfect_review(&review_path);
        let mut input: Value = serde_json::from_slice(&fs::read(&review_path).unwrap()).unwrap();
        input["samples"].as_array_mut().unwrap().push(json!({
            "sample_id": "sample-1-2",
            "score": 99,
            "reason": "Minor punctuation difference",
            "differences": [{"category": "punctuation", "explanation": "Missing comma"}]
        }));
        fs::write(&review_path, serde_json::to_vec(&input).unwrap()).unwrap();

        // Unequal sample scores catch integer division and aggregating only one review.
        let mut report = apply_review(&report_path, &review_path).unwrap();

        assert_eq!(report.status, RunStatus::Complete);
        assert_eq!(report.aggregates.llm.as_ref().unwrap().mean_score, 99.5);
        assert_eq!(report.aggregates.llm.as_ref().unwrap().scored_samples, 2);
        assert_eq!(report.meets_targets, Some(true));
        assert!(EvaluationReport::read(&report_path).is_ok());

        // A rounded mean within the report's tolerance can cross an exact threshold. Gates
        // must still describe the validated persisted mean when a later process reads it.
        report.aggregates.llm.as_mut().unwrap().mean_score = 99.49999999999997;
        report.gates.as_mut().unwrap().llm = false;
        report.meets_targets = Some(false);
        report.write(&report_path).unwrap();
        assert!(EvaluationReport::read(&report_path).is_ok());
    }

    #[test]
    fn review_gates_include_thresholds_and_reject_each_failing_side() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        let mut report = awaiting_report();
        let sample = &mut report.samples[0];
        let hypothesis = "使用 Other";
        sample.hypothesis = Some(hypothesis.to_string());
        sample.wer = Some(score_wer(&sample.reference, hypothesis));
        sample.proper_nouns = Some(score_proper_nouns(
            hypothesis,
            &sample.proper_noun_annotations,
        ));
        report.aggregates.wer.substitutions = 1;
        report.aggregates.wer.wer_percent = 50.0;
        report.aggregates.proper_nouns.matched_occurrences = 0;
        report.aggregates.proper_nouns.accuracy_percent = 0.0;
        write_perfect_review(&review_path);
        let mut input: Value = serde_json::from_slice(&fs::read(&review_path).unwrap()).unwrap();
        input["samples"][0]["score"] = json!(85);
        input["samples"][0]["reason"] = json!("Proper noun changed");
        input["samples"][0]["differences"] = json!([{
            "category": "proper_noun", "explanation": "Codex became Other"
        }]);
        fs::write(&review_path, serde_json::to_vec(&input).unwrap()).unwrap();

        // Each metric sits on its passing boundary first; moving just one threshold must
        // flip that gate and the overall result in both freshly written and loaded reports.
        for (max_wer, min_llm, min_proper_nouns, expected) in [
            (50.0, 85.0, 0.0, (true, true, true, true)),
            (49.0, 85.0, 0.0, (false, true, true, false)),
            (50.0, 86.0, 0.0, (true, false, true, false)),
            (50.0, 85.0, 1.0, (true, true, false, false)),
        ] {
            report.thresholds = Thresholds {
                max_wer_percent: max_wer,
                min_llm_score: min_llm,
                min_proper_noun_percent: min_proper_nouns,
            };
            report.write(&report_path).unwrap();
            let completed = apply_review(&report_path, &review_path).unwrap();
            let gates = completed.gates.as_ref().unwrap();
            assert_eq!(
                (
                    gates.wer,
                    gates.llm,
                    gates.proper_nouns,
                    completed.meets_targets.unwrap()
                ),
                expected
            );
            assert!(EvaluationReport::read(&report_path).is_ok());
        }
    }

    #[test]
    fn fractional_metrics_survive_report_round_trip_before_review() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        let mut report = awaiting_report();
        let reference = "one two three four five six seven eight nine ten eleven twelve Codex";
        let hypothesis =
            "changed words three four five six seven eight nine ten eleven twelve Codex";
        // Two errors over thirteen words reproduces the JSON float round trip that blocked
        // applying a review to a real prompt-lab report in a later process.
        let wer = score_wer(reference, hypothesis);
        report.thresholds.max_wer_percent = 20.0;
        report.aggregates.wer = WerAggregate {
            reference_words: wer.reference_words,
            substitutions: wer.substitutions,
            deletions: wer.deletions,
            insertions: wer.insertions,
            wer_percent: wer.wer_percent,
        };
        report.samples[0].reference = reference.to_string();
        report.samples[0].hypothesis = Some(hypothesis.to_string());
        report.samples[0].wer = Some(wer);
        report.write(&report_path).unwrap();
        write_perfect_review(&review_path);

        let report = apply_review(&report_path, &review_path).unwrap();

        assert_eq!(report.status, RunStatus::Complete);
        assert_eq!(report.meets_targets, Some(true));
        assert!(EvaluationReport::read(&report_path).is_ok());
    }

    #[test]
    fn exact_metric_tampering_is_rejected_without_rewriting_report() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        let mut report = awaiting_report();
        // Keep the aggregate consistent with the forged sample so only recomputation from the
        // reference/hypothesis can catch the structural metric tampering.
        let wer = report.samples[0].wer.as_mut().unwrap();
        wer.substitutions = 1;
        wer.wer_percent = 50.0;
        report.aggregates.wer.substitutions = 1;
        report.aggregates.wer.wer_percent = 50.0;
        report.write(&report_path).unwrap();
        let before = fs::read(&report_path).unwrap();
        write_perfect_review(&review_path);

        let error = apply_review(&report_path, &review_path).unwrap_err();

        assert!(error.to_string().contains("metrics do not match"));
        assert_eq!(fs::read(&report_path).unwrap(), before);
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
    fn invalid_review_payloads_are_rejected_at_both_json_boundaries() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        awaiting_report().write(&report_path).unwrap();
        let before = fs::read(&report_path).unwrap();
        write_perfect_review(&review_path);
        let input: Value = serde_json::from_slice(&fs::read(&review_path).unwrap()).unwrap();
        let completed = apply_review(&report_path, &review_path).unwrap();
        let completed = serde_json::to_value(completed).unwrap();

        // Users can edit review files and completed reports independently. Both boundaries
        // must reject the invalid payload itself, before aggregate checks or a report rewrite.
        for (field, value) in [
            ("score", json!(101)),
            ("reason", json!(" \t")),
            ("score", json!(99)),
            (
                "differences",
                json!([{"category": " ", "explanation": "difference"}]),
            ),
            (
                "differences",
                json!([{"category": "other", "explanation": " \t"}]),
            ),
        ] {
            let mut invalid_input = input.clone();
            invalid_input["samples"][0][field] = value.clone();
            fs::write(&report_path, &before).unwrap();
            fs::write(&review_path, serde_json::to_vec(&invalid_input).unwrap()).unwrap();
            let error = apply_review(&report_path, &review_path).unwrap_err();
            assert!(error.to_string().contains("sample-1-1"), "{field}: {error}");
            assert_eq!(fs::read(&report_path).unwrap(), before);

            let mut invalid_report = completed.clone();
            invalid_report["samples"][0]["agent_review"][field] = value;
            fs::write(&report_path, serde_json::to_vec(&invalid_report).unwrap()).unwrap();
            let error = EvaluationReport::read(&report_path).unwrap_err();
            assert!(error.to_string().contains("sample-1-1"), "{field}: {error}");
        }
    }

    #[test]
    fn completed_report_rejects_changed_review_aggregates_and_gates() {
        let directory = tempdir().unwrap();
        let report_path = directory.path().join("run.json");
        let review_path = directory.path().join("review.json");
        awaiting_report().write(&report_path).unwrap();
        write_perfect_review(&review_path);
        let completed = apply_review(&report_path, &review_path).unwrap();
        let completed = serde_json::to_value(completed).unwrap();

        // Stored derived values must be checked against the sample reviews and thresholds;
        // sharing their calculation must not make edited summary fields authoritative.
        for (pointer, value) in [
            ("/aggregates/llm/mean_score", json!(99.0)),
            ("/aggregates/llm/scored_samples", json!(2)),
            ("/gates/wer", json!(false)),
            ("/gates/llm", json!(false)),
            ("/gates/proper_nouns", json!(false)),
            ("/meets_targets", json!(false)),
        ] {
            let mut invalid = completed.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            fs::write(&report_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(EvaluationReport::read(&report_path).is_err(), "{pointer}");
        }
    }
}
