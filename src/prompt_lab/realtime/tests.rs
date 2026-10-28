use super::*;
use serde_json::json;

fn fixture() -> RealtimeCases {
    serde_json::from_value(json!({
        "schema_version": 2,
        "cases": [{"id":"project", "at":100, "seed":{}, "steps":[
            {"sample_id":"first"},
            {"sample_id":"second", "advance_seconds":86400}
        ]}]
    }))
    .unwrap()
}

#[test]
fn documented_seeded_learning_and_expiration_fixture_is_valid() {
    let cases = RealtimeCases::read(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/prompt-lab/realtime-cases.json"
    )))
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
        prompt_input: None,
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
fn a_topic_string_can_reuse_an_existing_snapshot_label_without_a_kind() {
    let snapshot: Snapshot = serde_json::from_value(json!({"contexts":[{
        "id":"project","label":"project","long_term":[],"candidates":[],"history":[]
    }]}))
    .unwrap();
    let traces = vec![SegmentTrace {
        prompt_input: None,
        index: 0,
        input: snapshot.clone(),
        output: json!({
            "text":"ready", "context":"project", "memory_candidates":[]
        }),
    }];
    assert!(
        Expectations::default()
            .check(&snapshot, &traces, &snapshot, "ready")
            .iter()
            .all(|check| check.passed)
    );
}

#[test]
fn ungrounded_candidates_fail_metadata_checks_even_when_text_is_valid() {
    let empty = Snapshot { contexts: vec![] };
    let traces = vec![SegmentTrace {
        prompt_input: Some(
            serde_json::from_value(json!({
                "stable":{"contexts":[]}, "dynamic":{"contexts":[]}
            }))
            .unwrap(),
        ),
        index: 0,
        input: empty.clone(),
        output: json!({
            "text":"使用Redis缓存。", "context":"开发",
            "memory_candidates":["Redis", "3核38都是无统的"]
        }),
    }];
    let learned = Replay::new(&MemorySeed::default(), 100)
        .replay("first", 100, &traces, true)
        .unwrap()
        .snapshot(100);
    let expected: Expectations = serde_json::from_value(json!({
        "context":"开发", "candidates_present":["Redis"],
        "candidates_absent":["3核38都是无统的"],
        "memory":{
            "history_present":["使用Redis缓存。"],
            "terms_present":[{"context":"开发", "kind":"proper_noun", "text":"Redis",
                "supporting_recordings":1}]
        }
    }))
    .unwrap();
    expected.validate().unwrap();
    let checks = expected.check(&empty, &traces, &learned, "使用Redis缓存。");
    assert!(
        checks
            .iter()
            .any(|check| check.name == "segment.0.metadata" && !check.passed)
    );
    assert!(
        checks
            .iter()
            .filter(|check| check.name != "segment.0.metadata")
            .all(|check| check.passed)
    );
}

fn metadata_passed(output: serde_json::Value) -> bool {
    let parsed = DictationResult::parse(&output.to_string()).unwrap();
    assert_eq!(Some(parsed.text.as_str()), output["text"].as_str());
    let empty = Snapshot { contexts: vec![] };
    Expectations::default()
        .check(
            &empty,
            &[SegmentTrace {
                prompt_input: None,
                index: 0,
                input: empty.clone(),
                output,
            }],
            &empty,
            "",
        )
        .into_iter()
        .find(|check| check.name == "segment.0.metadata")
        .unwrap()
        .passed
}

#[test]
fn context_assertions_round_trip_and_compare_exact_raw_strings() {
    // Runtime None also represents malformed metadata, so explicit unknown must check raw output.
    let empty = Snapshot { contexts: vec![] };
    for (wire, expected_context) in [
        (json!({}), None),
        (json!({"context": null}), None),
        (json!({"context": "未知语境"}), Some("未知语境")),
        (json!({"context": "project"}), Some("project")),
    ] {
        let expected: Expectations = serde_json::from_value(wire).unwrap();
        expected.validate().unwrap();
        let serialized = serde_json::to_value(&expected).unwrap();
        assert_eq!(serialized["context"].as_str(), expected_context);
        assert_eq!(
            serde_json::from_value::<Expectations>(serialized).unwrap(),
            expected
        );
        for actual in [
            json!(null),
            json!("未知语境"),
            json!("project"),
            json!("Project"),
        ] {
            let checks = expected.check(
                &empty,
                &[SegmentTrace {
                    prompt_input: None,
                    index: 0,
                    input: empty.clone(),
                    output: json!({"text":"ready", "context":actual, "memory_candidates":[]}),
                }],
                &empty,
                "ready",
            );
            let context = checks.iter().find(|check| check.name == "context");
            assert_eq!(context.is_some(), expected_context.is_some());
            if let Some(check) = context {
                assert_eq!(check.passed, actual.as_str() == expected_context);
            }
        }
    }
}

#[test]
fn strict_metadata_rejects_malformed_fields_without_rejecting_valid_text() {
    let valid = json!({
        "text":"Redis and Redis, then Codex.", "context":"project",
        "memory_candidates":["Redis", "Redis", "Codex"]
    });
    assert!(metadata_passed(valid.clone()));
    assert!(metadata_passed(json!({
        "text":"ready", "context":"未知语境", "memory_candidates":[]
    })));
    for context in [
        json!(null),
        json!(42),
        json!([]),
        json!({"kind":"new", "label":"project"}),
        json!(""),
    ] {
        let mut output = valid.clone();
        output["context"] = context;
        assert!(!metadata_passed(output), "invalid context accepted");
    }
    let mut missing_context = valid.clone();
    missing_context.as_object_mut().unwrap().remove("context");
    assert!(!metadata_passed(missing_context));

    // Partial grounding must not hide a malformed neighbor, incomplete duplicate group or overflow.
    for candidates in [
        json!(null),
        json!({}),
        json!("Codex"),
        json!([{"kind":"proper_noun", "text":"Codex", "quote":"then Codex."}]),
        json!(["Redis", "Redis", null, "Codex"]),
        json!(["Redis", "Redis", 42, "Codex"]),
        json!(["Redis", "Redis", "", "Codex"]),
        json!(["Redis", "Redis", "x".repeat(81), "Codex"]),
        json!(["Redis", "Redis", "absent", "Codex"]),
        json!(["Redis", "Codex"]),
    ] {
        let mut output = valid.clone();
        output["memory_candidates"] = candidates;
        assert!(!metadata_passed(output), "invalid candidates accepted");
    }
    for count in [8, 9] {
        assert_eq!(
            metadata_passed(json!({
                "text":"Codex ".repeat(count), "context":"project",
                "memory_candidates":vec!["Codex"; count]
            })),
            count == 8
        );
    }
    let mut missing_candidates = valid.clone();
    missing_candidates
        .as_object_mut()
        .unwrap()
        .remove("memory_candidates");
    assert!(!metadata_passed(missing_candidates));
    for candidates in [json!(["Codex"]), json!(null), json!({})] {
        assert!(!metadata_passed(json!({
            "text":"Codex", "context":"未知语境", "memory_candidates":candidates
        })));
    }
    assert!(!metadata_passed(json!({
        "text":"New York City", "context":"project",
        "memory_candidates":["New York", "York City"]
    })));
}

#[test]
fn capped_candidate_prefix_is_valid_without_authorizing_partial_name_groups() {
    // The prompt caps occurrences, not distinct names. A ninth mention must not
    // turn the correct eight-item prefix into a protocol error or a guessed edit.
    for (count, supplied, valid, retained) in [
        (7, 7, true, 7),
        (8, 8, true, 8),
        (9, 8, true, 0),
        (9, 7, false, 0),
        (9, 9, false, 0),
    ] {
        let output = json!({
            "text":"Codex ".repeat(count), "context":"project",
            "memory_candidates":vec!["Codex"; supplied]
        });
        assert_eq!(metadata_passed(output.clone()), valid, "{output}");
        assert_eq!(
            DictationResult::parse(&output.to_string())
                .unwrap()
                .memory_candidates
                .len(),
            retained
        );
    }

    let names = vec![
        "Redis", "Codex", "Codex", "Codex", "Codex", "Codex", "Codex", "Codex",
    ];
    let mut reordered = names.clone();
    reordered.swap(0, 1);
    let mut invented = names.clone();
    invented[7] = "absent";
    for (names, valid) in [(names, true), (reordered, false), (invented, false)] {
        let output = json!({
            "text":format!("Redis {}", "Codex ".repeat(8)), "context":"project",
            "memory_candidates":names
        });
        assert_eq!(metadata_passed(output.clone()), valid, "{output}");
        let parsed = DictationResult::parse(&output.to_string()).unwrap();
        assert_eq!(parsed.memory_candidates.len(), 1);
        assert_eq!(parsed.memory_candidates[0].text, "Redis");
    }

    assert!(metadata_passed(json!({
        "text":"New York; York; ".repeat(5), "context":"project",
        "memory_candidates":["New York", "York", "New York", "York", "New York", "York", "New York", "York"]
    })));
    for (text, names) in [
        (
            "New York City; ".repeat(5),
            vec![
                "New York",
                "York City",
                "New York",
                "York City",
                "New York",
                "York City",
                "New York",
                "York City",
            ],
        ),
        ("哈".repeat(12), vec!["哈哈"; 8]),
    ] {
        assert!(!metadata_passed(json!({
            "text":text, "context":"project", "memory_candidates":names
        })));
    }
}

#[test]
fn raw_candidate_order_is_checked_before_parser_sorting_can_hide_it() {
    // Counts alone accept this response after production sorts all three grounded occurrences.
    assert!(!metadata_passed(json!({
        "text":"Redis, Codex, Redis.", "context":"project",
        "memory_candidates":["Codex", "Redis", "Redis"]
    })));
    assert!(metadata_passed(json!({
        "text":"Redis, Codex, Redis.", "context":"project",
        "memory_candidates":["Redis", "Codex", "Redis"]
    })));
    assert!(metadata_passed(json!({
        "text":"Ｒｅｄｉｓ, CODEX.", "context":"project",
        "memory_candidates":["redis", "Codex"]
    })));
}

#[test]
fn old_cases_are_rejected_before_deserializing_legacy_assertions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cases.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version":1,
            "cases":[{"id":"case", "at":100, "steps":[{
                "sample_id":"first", "expect":{
                    "context":{"kind":"uncertain"},
                    "candidates_present":[{"kind":"proper_noun", "text":"Codex"}]
                }
            }]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let error = RealtimeCases::read(&path).unwrap_err().to_string();
    assert!(error.contains("schema_version") && error.contains('2'));
    assert!(error.contains("migrate") && error.contains("rerun"));
    let mut cases = fixture();
    cases.schema_version = 1;
    let error = cases
        .validate(&["first".into(), "second".into()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("migrate") && error.contains("rerun"));
}
