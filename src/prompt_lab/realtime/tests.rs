use super::*;
use serde_json::json;

fn fixture() -> RealtimeCases {
    serde_json::from_value(json!({
        "schema_version": 1,
        "cases": [{"id":"project", "at":100, "seed":{}, "steps":[
            {"sample_id":"first"},
            {"sample_id":"second", "advance_seconds":86400}
        ]}]
    }))
    .unwrap()
}

#[test]
fn documented_seeded_learning_and_expiration_fixture_is_valid() {
    let cases: RealtimeCases = serde_json::from_str(include_str!(
        "../../../tests/fixtures/prompt-lab/realtime-cases.json"
    ))
    .unwrap();
    cases.validate(&["first".into(), "second".into()]).unwrap();
}

#[test]
fn scenarios_require_exact_ready_coverage_before_network_io() {
    // Missing or repeated recordings would silently change metric weights in a regression run.
    let ready = vec!["first".to_string(), "second".to_string()];
    fixture().validate(&ready).unwrap();
    let mut missing = fixture();
    missing.cases[0].steps.pop();
    assert!(missing.validate(&ready).is_err());
    let mut duplicate = fixture();
    duplicate.cases[0].steps[1].sample_id = "first".into();
    assert!(duplicate.validate(&ready).is_err());
    let mut overflow = fixture();
    overflow.cases[0].at = u64::MAX;
    assert!(overflow.validate(&ready).is_err());
}

#[test]
fn comparison_identity_includes_order_and_assertions_but_allows_different_seeds() {
    let original = fixture();
    let mut seeded = fixture();
    seeded.cases[0]
        .seed
        .long_term
        .push(crate::dictation::config::LongTerm {
            context: "project".into(),
            kind: crate::dictation::config::TermKind::ProperNoun,
            text: "ViberWhisper".into(),
        });
    assert_eq!(
        original.contract_digest().unwrap(),
        seeded.contract_digest().unwrap()
    );
    assert_ne!(original.digest().unwrap(), seeded.digest().unwrap());
    let report = |cases: RealtimeCases| RealtimeReport {
        policy_version: POLICY.into(),
        cases_sha256: cases.digest().unwrap(),
        contract_sha256: cases.contract_digest().unwrap(),
        cases,
        steps: vec![],
    };
    let old = report(original.clone());
    let current = report(seeded.clone());
    let mut changes = current.seed_changes(&old);
    assert_eq!(changes.len(), 1);
    current.validate_seed_changes(&changes, true).unwrap();
    assert!(current.validate_seed_changes(&changes, false).is_err());
    changes[0].after = MemorySeed::default();
    assert!(current.validate_seed_changes(&changes, true).is_err());
    let mut changed_assertion = seeded.clone();
    changed_assertion.cases[0].steps[0]
        .expect
        .forbidden_text
        .push("history-only".into());
    assert_ne!(
        original.contract_digest().unwrap(),
        changed_assertion.contract_digest().unwrap()
    );
    seeded.cases[0].steps.swap(0, 1);
    assert_ne!(
        original.contract_digest().unwrap(),
        seeded.contract_digest().unwrap()
    );
}

#[test]
fn discarded_metadata_and_history_leaks_fail_local_checks() {
    // Production preserves valid text despite malformed metadata; evaluation must still expose it.
    let expected: Expectations = serde_json::from_value(json!({
        "forbidden_text":["old secret"]
    }))
    .unwrap();
    let segments = vec![crate::dictation::SegmentTrace {
        index: 0,
        input: crate::dictation::Snapshot { contexts: vec![] },
        output: json!({"text":"old secret", "context":{"kind":"existing","id":"invented"},
            "memory_candidates":[{"kind":"proper_noun","text":"invented"}]}),
    }];
    let checks = expected.check(
        &crate::dictation::Snapshot { contexts: vec![] },
        &segments,
        &crate::dictation::Snapshot { contexts: vec![] },
        "old secret",
    );
    assert!(
        checks
            .iter()
            .any(|c| c.name.contains("metadata") && !c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.name.contains("forbidden_text") && !c.passed)
    );
}

#[test]
fn new_topic_cannot_redeclare_an_existing_snapshot_label() {
    let snapshot: Snapshot = serde_json::from_value(json!({"contexts":[{
        "id":"project","label":"project","long_term":[],"candidates":[],"history":[]
    }]}))
    .unwrap();
    let traces = vec![SegmentTrace {
        index: 0,
        input: snapshot.clone(),
        output: json!({
            "text":"ready","context":{"kind":"new","label":"project"},"memory_candidates":[]
        }),
    }];
    assert!(
        Expectations::default()
            .check(&snapshot, &traces, &snapshot, "ready")
            .iter()
            .any(|check| !check.passed)
    );
}

#[test]
fn ungrounded_candidates_fail_metadata_checks_even_when_text_is_valid() {
    let empty = Snapshot { contexts: vec![] };
    let traces = vec![SegmentTrace {
        index: 0,
        input: empty.clone(),
        output: json!({
            "text":"使用Redis缓存。","context":{"kind":"new","label":"开发"},
            "memory_candidates":[{"kind":"proper_noun","text":"Redis"},
                {"kind":"phrase","text":"3核38都是无统的"}]
        }),
    }];
    let learned = Replay::new(&MemorySeed::default(), 100)
        .replay("first", 100, &traces, true)
        .unwrap()
        .snapshot(100);
    let checks = Expectations::default().check(&empty, &traces, &learned, "使用Redis缓存。");
    assert!(
        checks
            .iter()
            .any(|check| check.name == "segment.0.metadata" && !check.passed)
    );
}
