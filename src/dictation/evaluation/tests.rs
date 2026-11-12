use super::*;
use crate::dictation::DictationResult;
use serde_json::json;

fn seeded() -> MemorySeed {
    serde_json::from_value(
        json!({"long_term":[{"context":"project","kind":"proper_noun","text":"ViberWhisper"}]}),
    )
    .unwrap()
}

#[test]
fn replay_preserves_support_counts_expiration_and_failed_recording_rollback() {
    let seed = seeded();
    let state = Replay::new(&seed, 100);
    let trace = SegmentTrace {
        prompt_input: Some(
            state
                .memory
                .prompt_input(&state.memory.stable_prefix(), 100),
        ),
        index: 0,
        input: Some(state.snapshot(100)),
        input_layout: super::super::INPUT_LAYOUT.into(),
        at: None,
        instructions: None,
        audio_start_sample: None,
        audio_end_sample: None,
        output: json!({
            "text":"ViberWhisper ready", "context":"project",
            "memory_candidates":["ViberWhisper"]
        }),
    };
    let learned = state
        .replay("first", 100, std::slice::from_ref(&trace), true)
        .unwrap();
    let snapshot = learned.snapshot(100);
    // Simpler frozen-snapshot evaluation sees identical initial input but cannot validate learning.
    let mut frozen = Memory::frozen(MemoryConfig {
        enabled: true,
        long_term: seed.long_term.clone(),
    });
    assert_eq!(frozen.snapshot(100), state.snapshot(100));
    let result = super::super::result::DictationResult::parse(&trace.output.to_string()).unwrap();
    frozen.learn("sample:first", 0, &result, 100);
    assert!(frozen.snapshot(100).contexts[0].candidates.is_empty());
    assert!(frozen.snapshot(100).contexts[0].history.is_empty());
    // Reports must survive serialization including flattened supported-term fields.
    assert_eq!(
        serde_json::from_value::<Snapshot>(serde_json::to_value(&snapshot).unwrap()).unwrap(),
        snapshot
    );
    assert_eq!(snapshot.contexts[0].candidates[0].supporting_recordings, 1);
    let second = SegmentTrace {
        prompt_input: Some(
            learned
                .memory
                .prompt_input(&learned.memory.stable_prefix(), 100),
        ),
        input: Some(snapshot),
        ..trace.clone()
    };
    let twice = learned
        .replay("second", 100, std::slice::from_ref(&second), true)
        .unwrap();
    assert_eq!(
        twice.snapshot(100).contexts[0].candidates[0].supporting_recordings,
        2
    );
    assert_eq!(
        learned
            .replay("second", 100, &[second], false)
            .unwrap()
            .snapshot(100),
        learned.snapshot(100)
    );
    let expired = twice.snapshot(86500);
    assert_eq!(expired.contexts[0].long_term, seed.long_term);
    assert!(expired.contexts[0].history.is_empty());
    assert!(expired.contexts[0].candidates.is_empty());
    let mut missing_input = trace.clone();
    missing_input.prompt_input = None;
    assert!(state.replay("first", 100, &[missing_input], true).is_err());
    let mut tampered = trace;
    tampered.input.as_mut().unwrap().contexts.clear();
    assert!(state.replay("first", 100, &[tampered], true).is_err());
}

#[test]
fn replay_uses_actual_segment_prompt_times_when_history_expires_mid_recording() {
    let seed = seeded();
    let state = Replay::new(&seed, 100);
    let input = state
        .memory
        .prompt_input(&state.memory.stable_prefix(), 100);
    let first = SegmentTrace {
        index: 0,
        at: Some(100),
        input: Some(input.snapshot()),
        prompt_input: Some(input),
        input_layout: super::super::INPUT_LAYOUT.into(),
        instructions: None,
        audio_start_sample: None,
        audio_end_sample: None,
        output: json!({"text":"first", "context":"project", "memory_candidates":[]}),
    };
    let transient = state
        .replay("recording", 100, std::slice::from_ref(&first), true)
        .unwrap();
    let input = transient
        .memory
        .prompt_input(&transient.memory.stable_prefix(), 86_500);
    let second = SegmentTrace {
        index: 1,
        at: Some(86_500),
        input: Some(input.snapshot()),
        prompt_input: Some(input),
        output: json!({"text":"second", "context":"project", "memory_candidates":[]}),
        ..first.clone()
    };
    let learned = state
        .replay("recording", 100, &[first, second], true)
        .unwrap();
    assert_eq!(learned.snapshot(86_500).contexts[0].history, vec!["second"]);
}

#[test]
fn seed_budget_is_checked_before_production_trimming() {
    let mut seed = MemorySeed::default();
    for context in 0..4 {
        for term in 0..16 {
            seed.long_term.push(super::super::config::LongTerm {
                context: format!("context-{context}"),
                kind: super::super::config::TermKind::Phrase,
                text: format!("{term}{}", "词".repeat(70)),
            });
        }
    }
    assert!(seed.validate(100).is_err());
}

#[test]
fn seed_validation_rejects_observations_expiring_before_the_first_request() {
    // A scenario must not claim to seed history that production drops on initialization.
    let seed = MemorySeed {
        observations: vec![Observation::new(
            "prior-recording",
            0,
            100,
            "project",
            "历史",
            vec![],
        )],
        ..Default::default()
    };
    seed.validate(100 + 86_399).unwrap();
    assert!(
        !Replay::new(&seed, 100 + 86_399)
            .snapshot(100 + 86_399)
            .history_is_empty()
    );
    assert!(seed.validate(100 + 86_400).is_err());
}

fn correction_result(context: &str) -> DictationResult {
    DictationResult::parse(
        &json!({"text":"Use Cody.", "context":context, "memory_candidates":["Cody"]}).to_string(),
    )
    .unwrap()
}

#[test]
fn capture_preserves_complete_production_state_and_only_the_temp_copy_learns() {
    // Real storage retains more contexts, history and correction rules than the prompt can show.
    // Capturing that storage must not impose the artificial scenario's four-context limit.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.json");
    let destination = directory.path().join("isolated/memory.json");
    let settings = MemoryConfig {
        enabled: true,
        long_term: (0..5)
            .map(|i| LongTerm {
                context: format!("project-{i}"),
                kind: crate::dictation::config::TermKind::ProperNoun,
                text: format!("Configured{i}"),
            })
            .collect(),
    };
    settings.validate().unwrap();
    let mut memory = Memory::load(Some(source.clone()), settings.clone(), 0).unwrap();
    memory.learn("expired", 0, &correction_result("expired"), 0);
    for i in 0..5 {
        let prepared = memory.prepare(vec![correction_result(&format!("project-{i}"))], None);
        memory.confirm(&format!("confirmed-{i}"), &prepared, "Use Codex.", 100);
    }
    for i in 0..24 {
        let prepared = memory.prepare(vec![correction_result("project-0")], None);
        memory.confirm(&format!("history-{i}"), &prepared, &prepared.text, 100);
    }
    memory.save().unwrap();
    let bytes = std::fs::read(&source).unwrap();

    let at = 86_400;
    let captured = MemorySeed::capture(&source, settings.clone(), at);
    assert_eq!(captured.long_term, settings.long_term);
    assert_eq!(captured.observations.len(), 24); // Twenty in project-0, one in each other context.
    assert!(captured.observations.iter().all(|o| o.at == 100));
    assert_eq!(captured.corrections.len(), 5);
    assert_eq!(
        serde_json::from_slice::<MemorySeed>(&serde_json::to_vec(&captured).unwrap()).unwrap(),
        captured
    );
    assert!(captured.validate(at).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);

    captured.write(&destination, at).unwrap();
    assert_eq!(
        MemorySeed::capture(&destination, settings.clone(), at),
        captured
    );
    let mut isolated = Memory::load(Some(destination.clone()), settings.clone(), at).unwrap();
    let mut replay = Replay::production(&captured, at, true);
    assert_eq!(isolated.snapshot(at), replay.snapshot(at));
    assert!(!replay.snapshot(at).ids().contains(&"project-4".into()));
    let raw = correction_result("project-4");
    let prepared = isolated.prepare(vec![raw.clone()], None);
    assert_eq!(prepared.text, "Use Codex.");
    assert_eq!(
        replay.prepare_and_confirm("lab", vec![raw.clone()], None, Some("Use Codix."), at),
        prepared.text
    );
    isolated.confirm("lab", &prepared, "Use Codix.", at);
    isolated.save().unwrap();
    assert_eq!(isolated.snapshot(at), replay.snapshot(at));
    let learned = MemorySeed::capture(&destination, settings.clone(), at);
    assert_eq!(learned.corrections.len(), 5);
    assert_eq!(
        learned.memory(at).prepare(vec![raw], None).text,
        "Use Codix."
    );
    assert_ne!(learned, captured);
    assert_eq!(MemorySeed::capture(&source, settings.clone(), at), captured);
    let disabled = MemorySeed::capture(
        &source,
        MemoryConfig {
            enabled: false,
            ..settings
        },
        at,
    );
    assert_eq!(disabled.long_term, captured.long_term);
    assert!(disabled.observations.is_empty() && disabled.corrections.is_empty());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
}

#[test]
fn capture_and_production_recovery_preserve_broken_or_missing_sources() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.json");
    let expected = seeded();
    let settings = MemoryConfig {
        enabled: true,
        long_term: expected.long_term.clone(),
    };
    for bytes in ["broken JSON", r#"{"schema_version":3,"observations":[]}"#] {
        std::fs::write(&source, bytes).unwrap();
        assert!(Memory::load(Some(source.clone()), settings.clone(), 100).is_err());
        assert_eq!(
            MemorySeed::capture(&source, settings.clone(), 100),
            expected
        );
        let mut recovered = Memory::load_or_ephemeral(Some(source.clone()), settings.clone(), 100);
        let prepared = recovered.prepare(vec![correction_result("project")], None);
        recovered.confirm("ephemeral", &prepared, "Use Codex.", 100);
        recovered.save().unwrap();
        assert!(!recovered.snapshot(100).history_is_empty());

        // The fallible loader succeeds on broken JSON only if disabled memory skips the read.
        let disabled = MemoryConfig {
            enabled: false,
            ..settings.clone()
        };
        assert!(Memory::load(Some(source.clone()), disabled.clone(), 100).is_ok());
        assert_eq!(MemorySeed::capture(&source, disabled, 100), expected);
        assert_eq!(std::fs::read(&source).unwrap(), bytes.as_bytes());
    }
    let missing = directory.path().join("missing.json");
    assert_eq!(MemorySeed::capture(&missing, settings, 100), expected);
    assert!(!missing.exists());
}

#[test]
fn historical_seed_bytes_stay_stable_and_scenario_corrections_are_validated() {
    let bytes = r#"{"long_term":[],"observations":[]}"#;
    let mut seed: MemorySeed = serde_json::from_str(bytes).unwrap();
    assert!(seed.corrections.is_empty());
    assert_eq!(serde_json::to_string(&seed).unwrap(), bytes);
    let mut memory = seed.memory(100);
    let prepared = memory.prepare(vec![correction_result("project")], None);
    memory.confirm("confirmed", &prepared, "Use Codex.", 100);
    seed = memory.into_seed();
    seed.validate(100).unwrap();
    seed.corrections.push(seed.corrections[0].clone());
    assert!(seed.validate(100).is_err());
    seed.corrections.pop();
    seed.corrections[0].rule.to = "Cody".into();
    assert!(seed.validate(100).is_err());
}
