use super::*;
use crate::dictation::{config::MemoryConfig, memory::Memory};
use serde_json::json;

fn result(text: &str, term: &str, context: &str) -> DictationResult {
    DictationResult::parse(
        &json!({
            "text": text, "context": {"kind":"new", "label":context},
            "memory_candidates":[{"kind":"proper_noun", "text":term}]
        })
        .to_string(),
        &[],
    )
    .unwrap()
}

#[test]
fn confirmed_name_correction_survives_restart_without_reinforcing_automatic_edits() {
    // A user fixes a project name once. Confirming a later automatic correction
    // must preserve that evidence rather than count the program's output as a new edit.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let mut memory = Memory::load(Some(path.clone()), MemoryConfig::default(), 100).unwrap();
    let raw = result("继续开发维伯耳语。", "维伯耳语", "项目");
    let prepared = memory.prepare(vec![raw.clone()], Some("zh".into()));
    assert_eq!(prepared.text, raw.text);
    memory.confirm("first", &prepared, "继续开发ViberWhisper。", 100);
    let once = memory.snapshot(100);
    memory.confirm("first", &prepared, "继续开发OtherProject。", 100);
    assert_eq!(
        memory.snapshot(100),
        once,
        "duplicate feedback must be ignored"
    );
    memory.save().unwrap();
    let mut memory = Memory::load(Some(path), MemoryConfig::default(), 100_000).unwrap();
    let prepared = memory.prepare(vec![raw], Some("zh".into()));
    assert_eq!(prepared.text, "继续开发ViberWhisper。");
    let evidence = memory.snapshot(100_000).contexts[0].corrections.clone();
    memory.confirm("automatic", &prepared, &prepared.text, 100_000);
    assert_eq!(memory.snapshot(100_000).contexts[0].corrections, evidence);
    let snapshot = serde_json::to_value(memory.snapshot(100_000)).unwrap();
    assert_eq!(
        snapshot["contexts"][0]["candidates"][0]["text"],
        "ViberWhisper"
    );
}

#[test]
fn corrections_use_whole_names_and_revoke_a_reverted_automatic_replacement() {
    // Learning only the changed Han character would alter unrelated words.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let raw = result("请找张山。", "张山", "同事");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("first", &prepared, "请找张珊。", 100);
    let prepared = memory.prepare(vec![raw.clone()], None);
    assert_eq!(prepared.text, "请找张珊。");
    assert_eq!(
        memory
            .prepare(vec![result("去登山。", "登山", "同事")], None)
            .text,
        "去登山。"
    );
    memory.confirm("third-spelling", &prepared, "请找张三。", 101);
    let prepared = memory.prepare(vec![raw.clone()], None);
    assert_eq!(prepared.text, "请找张三。");
    memory.confirm("revert", &prepared, &raw.text, 102);
    assert_eq!(memory.prepare(vec![raw], None).text, "请找张山。");
}

#[test]
fn explicit_reversions_precede_name_boundary_heuristics() {
    // Returning to a known raw spelling is an undo, even when it adds a word or
    // contains the automatic name. Punctuation must not hide that explicit undo.
    for (original, corrected) in [
        ("Open Eye", "OpenAI"),
        ("Call Codex", "Codex"),
        ("Acme Co.", "ACME"),
        ("C++ Studio", "C++"),
    ] {
        for (prefix, suffix) in [("", ""), ("Use ", " today.")] {
            for punctuation in ["", "!"] {
                let mut memory = Memory::load(None, Default::default(), 100).unwrap();
                let raw = result(&format!("{prefix}{original}{suffix}"), original, "项目");
                let prepared = memory.prepare(vec![result(original, original, "项目")], None);
                memory.confirm("initial", &prepared, corrected, 100);
                let prepared = memory.prepare(vec![raw.clone()], None);
                assert_eq!(prepared.text, format!("{prefix}{corrected}{suffix}"));
                let evidence = memory.snapshot(100).contexts[0].corrections.clone();
                memory.confirm("unchanged", &prepared, &prepared.text, 101);
                assert_eq!(memory.snapshot(101).contexts[0].corrections, evidence);
                let edited = format!("{prefix}{original}{punctuation}{suffix}");
                memory.confirm("undo", &prepared, &edited, 102);
                assert!(
                    memory.snapshot(102).contexts[0].corrections.is_empty(),
                    "undo: {edited}"
                );
                assert_eq!(memory.prepare(vec![raw.clone()], None).text, raw.text);
            }
        }
    }
}

#[test]
fn explicit_reversions_restore_original_segment_boundaries() {
    // A restored suffix word can land beyond the displayed segment's end. Raw
    // source ranges retain its owner even if another segment stays auto-corrected.
    for revert_second in [false, true] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        for (id, original, corrected) in
            [("first", "C++ Studio", "C++"), ("second", "Ready", "Done")]
        {
            let prepared = memory.prepare(vec![result(original, original, "项目")], None);
            memory.confirm(id, &prepared, corrected, 100);
        }
        let raw = vec![
            result("C++ Studio", "C++ Studio", "项目"),
            result("Ready", "Ready", "项目"),
        ];
        let prepared = memory.prepare(raw.clone(), None);
        assert_eq!(prepared.text, "C++ Done");
        let edited = if revert_second {
            "C++ Studio Ready"
        } else {
            "C++ Studio Done"
        };
        memory.confirm("undo", &prepared, edited, 101);
        assert_eq!(memory.prepare(raw, None).text, edited);
        assert_eq!(
            memory.snapshot(101).contexts[0].corrections.len(),
            usize::from(!revert_second)
        );
    }
}

#[test]
fn neighboring_names_cannot_be_mistaken_for_restored_words() {
    // Raw C++ Studio contains its neighbor's displayed Studio. Adding punctuation
    // to that neighbor must not undo C++ Studio -> C++, in one or two segments.
    let mut combined = result("C++ Studio Studio", "C++ Studio", "项目");
    combined.memory_candidates.push(Candidate {
        kind: TermKind::ProperNoun,
        text: "Studio".into(),
    });
    for raw in [
        vec![combined],
        vec![
            result("C++ Studio", "C++ Studio", "项目"),
            result("Studio", "Studio", "项目"),
        ],
    ] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let prepared = memory.prepare(vec![result("C++ Studio", "C++ Studio", "项目")], None);
        memory.confirm("initial", &prepared, "C++", 100);
        let prepared = memory.prepare(raw.clone(), None);
        assert_eq!(prepared.text, "C++ Studio");
        let evidence = memory.snapshot(100).contexts[0].corrections.clone();
        memory.confirm("punctuation", &prepared, "C++ Studio!", 101);
        assert_eq!(memory.snapshot(101).contexts[0].corrections, evidence);
        assert_eq!(memory.prepare(raw, None).text, "C++ Studio");
    }
}

#[test]
fn automatic_corrections_require_a_complete_model_proper_noun() {
    // A known error elsewhere in the transcript is not permission to edit it:
    // each response must identify that whole name as a proper-noun candidate.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let raw = result("用Cody。", "Cody", "项目");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("first", &prepared, "用Codex。", 100);
    assert_eq!(memory.prepare(vec![raw.clone()], None).text, "用Codex。");

    let mut missing = raw.clone();
    missing.memory_candidates.clear();
    let mut phrase = raw;
    phrase.memory_candidates[0].kind = TermKind::Phrase;
    for source in [
        missing,
        phrase,
        result("用Cody和Redis。", "Redis", "项目"),
        result("去New Cody。", "New Cody", "项目"),
    ] {
        let expected = source.text.clone();
        assert_eq!(memory.prepare(vec![source], None).text, expected);
    }
}

#[test]
fn overlapping_model_names_do_not_learn_fragment_corrections() {
    // Models can extract both a full name and its last character. Learning the
    // nested fragment would later alter unrelated names or ordinary words.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let mut raw = result("请找张山。", "张山", "同事");
    raw.memory_candidates.push(Candidate {
        kind: TermKind::ProperNoun,
        text: "山".into(),
    });
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("first", &prepared, "请找张珊。", 100);
    assert_eq!(
        memory.snapshot(100).contexts[0].corrections,
        [Replacement {
            from: "张山".into(),
            to: "张珊".into(),
        }]
    );
    assert_eq!(memory.prepare(vec![raw], None).text, "请找张珊。");
    assert_eq!(
        memory
            .prepare(vec![result("去登山。", "山", "同事")], None)
            .text,
        "去登山。"
    );
}

#[test]
fn crossing_model_names_cannot_choose_a_correction_span() {
    // Neither candidate covers the complete overlap, so the user changing York
    // must not teach two incompatible guesses about the intended full name.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let mut raw = result("New York City", "New York", "地点");
    raw.memory_candidates.push(Candidate {
        kind: TermKind::ProperNoun,
        text: "York City".into(),
    });
    let prepared = memory.prepare(vec![raw], None);
    memory.confirm("first", &prepared, "New Yolk City", 100);
    assert!(memory.snapshot(100).contexts[0].corrections.is_empty());
}

#[test]
fn configured_phrases_protect_their_full_span_from_candidate_corrections() {
    // York remains a model candidate, but a saved York -> Yolk correction must
    // leave the configured New York intact while correcting a standalone York.
    let settings = MemoryConfig {
        enabled: true,
        long_term: vec![super::super::config::LongTerm {
            context: "地点".into(),
            kind: TermKind::ProperNoun,
            text: "New York".into(),
        }],
    };
    let mut memory = Memory::load(None, settings, 100).unwrap();
    let prepared = memory.prepare(vec![result("York", "York", "地点")], None);
    memory.confirm("first", &prepared, "Yolk", 100);
    let raw = result("New York; York", "York", "地点");
    let prepared = memory.prepare(vec![raw.clone()], None);
    assert_eq!(prepared.text, "New York; Yolk");
    // Confirming a protected occurrence must not revoke the independently usable rule.
    memory.confirm("unchanged", &prepared, &prepared.text, 101);
    assert_eq!(memory.prepare(vec![raw], None).text, "New York; Yolk");
    assert_eq!(
        memory
            .prepare(vec![result("York", "York", "地点")], None)
            .text,
        "Yolk"
    );
}

#[test]
fn configured_mentions_do_not_train_fragment_rules() {
    // Protection also applies before any rules exist: editing a configured place
    // name must not train a generic correction for its separately extracted part.
    let settings = MemoryConfig {
        enabled: true,
        long_term: vec![super::super::config::LongTerm {
            context: "地点".into(),
            kind: TermKind::ProperNoun,
            text: "New York".into(),
        }],
    };
    let mut memory = Memory::load(None, settings, 100).unwrap();
    let prepared = memory.prepare(vec![result("New York", "York", "地点")], None);
    memory.confirm("edited", &prepared, "New Yolk", 100);
    assert!(memory.snapshot(100).contexts[0].corrections.is_empty());
    assert_eq!(
        memory
            .prepare(vec![result("York", "York", "地点")], None)
            .text,
        "York"
    );
}

#[test]
fn unrelated_contexts_word_substrings_and_editorial_changes_do_not_become_rules() {
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let prepared = memory.prepare(vec![result("用Cody。", "Cody", "项目")], None);
    memory.confirm("first", &prepared, "用Codex。", 100);
    for (text, context) in [
        ("用Cody。", "闲聊"),
        ("用Codybase。", "项目"),
        ("用éCody。", "项目"),
    ] {
        assert_eq!(
            memory
                .prepare(vec![result(text, "Cody", context)], None)
                .text,
            text
        );
    }
    for (raw, final_text) in [
        (result("请找张山。", "张山", "同事"), "明天再说。"),
        (result("今天用Redis。", "Redis", "项目"), "今天用Redis！"),
        (result("先试123。", "123", "项目"), "先试124。"),
    ] {
        let prepared = memory.prepare(vec![raw], None);
        memory.confirm("editorial", &prepared, final_text, 101);
    }
    let corrections: usize = memory
        .snapshot(101)
        .contexts
        .iter()
        .map(|c| c.corrections.len())
        .sum();
    assert_eq!(corrections, 1);
}

#[test]
fn multiple_segments_keep_their_topics_and_final_unicode_text() {
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let prepared = memory.prepare(
        vec![
            result("联系张山。", "张山", "同事"),
            result("开发维伯耳语🎙️。", "维伯耳语", "项目"),
        ],
        Some("zh".into()),
    );
    memory.confirm(
        "recording",
        &prepared,
        "联系张珊。开发ViberWhisper🎙️。",
        100,
    );
    let snapshot = memory.snapshot(100);
    assert_eq!(snapshot.contexts.len(), 2);
    let colleague = snapshot
        .contexts
        .iter()
        .find(|c| c.label == "同事")
        .unwrap();
    assert_eq!(colleague.history, ["联系张珊。"]);
    let project = snapshot
        .contexts
        .iter()
        .find(|c| c.label == "项目")
        .unwrap();
    assert_eq!(project.history, ["开发ViberWhisper🎙️。"]);
    assert_eq!(project.corrections[0].to, "ViberWhisper");
}

#[test]
fn disabled_memory_and_discarded_previews_do_not_learn() {
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let raw = result("用维伯耳语。", "维伯耳语", "项目");
    let _discarded = memory.prepare(vec![raw.clone()], None);
    assert!(memory.snapshot(100).contexts.is_empty());
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("confirmed", &prepared, "用ViberWhisper。", 100);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disabled.json");
    std::fs::write(&path, b"not JSON").unwrap();
    let mut disabled = Memory::load(
        Some(path.clone()),
        MemoryConfig {
            enabled: false,
            long_term: vec![],
        },
        100,
    )
    .unwrap();
    let prepared = disabled.prepare(vec![raw], None);
    disabled.confirm("ignored", &prepared, "用ViberWhisper。", 100);
    disabled.save().unwrap();
    assert!(disabled.snapshot(100).contexts.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), b"not JSON");
}

#[test]
fn replacement_is_single_pass_and_cycles_are_suppressed() {
    let rules = [("Alpha", "Beta"), ("Beta", "Gamma")].map(|(from, to)| Correction {
        context: "项目".into(),
        rule: Replacement {
            from: from.into(),
            to: to.into(),
        },
        recording: "first".into(),
        at: 100,
    });
    let mut raw = result("Alpha Beta", "Alpha", "项目");
    assert_eq!(
        Prepared::new(vec![raw.clone()], &rules, &[], None).text,
        "Beta Beta"
    );
    raw.memory_candidates.push(Candidate {
        kind: TermKind::ProperNoun,
        text: "Beta".into(),
    });
    assert_eq!(
        Prepared::new(vec![raw.clone()], &rules, &[], None).text,
        "Beta Gamma"
    );
    let mut cycle = rules.to_vec();
    cycle[1].rule.to = "Alpha".into();
    assert_eq!(
        Prepared::new(vec![raw], &cycle, &[], None).text,
        "Alpha Beta"
    );
}

#[test]
fn migration_limits_and_manual_vocabulary_preserve_the_memory_contract() {
    // Existing model-only memory must load without being promoted to trusted
    // corrections; learned rules must not displace manually configured terms.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    std::fs::write(
        &path,
        json!({"schema_version":1, "observations":[]}).to_string(),
    )
    .unwrap();
    let mut memory = Memory::load(Some(path.clone()), Default::default(), 100).unwrap();
    let raw = result("请找张山。", "张山", "同事");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("first", &prepared, "请找张珊。", 100);
    memory.save().unwrap();
    let earlier_clock = Memory::load(Some(path.clone()), Default::default(), 50).unwrap();
    assert_eq!(
        earlier_clock.prepare(vec![raw.clone()], None).text,
        "请找张珊。"
    );
    let settings = MemoryConfig {
        enabled: true,
        long_term: vec![super::super::config::LongTerm {
            context: "同事".into(),
            kind: super::super::config::TermKind::ProperNoun,
            text: "张山".into(),
        }],
    };
    let memory = Memory::load(Some(path.clone()), settings, 100).unwrap();
    assert_eq!(memory.prepare(vec![raw], None).text, "请找张山。");
    assert!(memory.snapshot(100).contexts[0].corrections.is_empty());

    let rules: Vec<_> = (0..520)
        .map(|i| Correction {
            context: "项目".into(),
            rule: Replacement {
                from: format!("wrong{i}"),
                to: format!("right{i}"),
            },
            recording: format!("recording-{i}"),
            at: i,
        })
        .collect();
    std::fs::write(
        &path,
        json!({"schema_version":2, "observations":[], "corrections":rules}).to_string(),
    )
    .unwrap();
    let memory = Memory::load(Some(path.clone()), Default::default(), 1000).unwrap();
    memory.save().unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored["corrections"].as_array().unwrap().len(), 512);
    assert_eq!(stored["corrections"][0]["rule"]["from"], "wrong8");
    assert!(
        serde_json::to_string(&memory.snapshot(1000))
            .unwrap()
            .chars()
            .count()
            <= 6000
    );
    for contents in ["broken JSON", r#"{"schema_version":3,"observations":[]}"#] {
        std::fs::write(&path, contents).unwrap();
        assert!(Memory::load(Some(path.clone()), Default::default(), 1000).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }
}

#[test]
fn ambiguous_and_expensive_edits_cannot_assign_terms_to_a_guessed_span() {
    // Reordered matching anchors admit multiple explanations; a large rewrite
    // must also stop within a fixed alignment budget.
    assert!(alignment::Alignment::new("前abc后", "前bac后").is_none());
    assert!(alignment::Alignment::new(&"a".repeat(1000), &"b".repeat(1000)).is_none());
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let mut raw = result("只用旧名称。", "旧名称", "项目");
    raw.context = None;
    raw.memory_candidates.clear();
    let prepared = memory.prepare(vec![raw], None);
    memory.confirm("uncertain", &prepared, "只用新名称。", 100);
    assert!(memory.snapshot(100).contexts.is_empty());
}

#[test]
fn different_final_spellings_revoke_a_rule_instead_of_choosing_one_occurrence() {
    // The same model word may refer to different names. Changing only one of two
    // automatic replacements must not force that new spelling onto both next time.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let prepared = memory.prepare(vec![result("联系张山。", "张山", "同事")], None);
    memory.confirm("first", &prepared, "联系张珊。", 100);
    let raw = result("联系张山，再联系张山。", "张山", "同事");
    let prepared = memory.prepare(vec![raw.clone()], None);
    assert_eq!(prepared.text, "联系张珊，再联系张珊。");
    memory.confirm("different", &prepared, "联系张三，再联系张珊。", 101);
    assert_eq!(
        memory.prepare(vec![raw], None).text,
        "联系张山，再联系张山。"
    );
}

#[test]
fn unchanged_occurrences_prevent_learning_a_conflicting_spelling() {
    // The two mentions may name different colleagues. Keeping one spelling is
    // conflicting evidence even when it appears in a separate, repeated segment.
    for unchanged in ["张山正在开会。", "张山和张山正在开会。"] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let sources = vec![
            result("请找张山。", "张山", "同事"),
            result(unchanged, "张山", "同事"),
        ];
        let prepared = memory.prepare(sources.clone(), Some("zh".into()));
        memory.confirm("mixed", &prepared, &format!("请找张珊。{unchanged}"), 100);
        assert!(memory.snapshot(100).contexts[0].corrections.is_empty());
        assert_eq!(
            memory.prepare(sources, Some("zh".into())).text,
            format!("请找张山。{unchanged}")
        );
    }
}

#[test]
fn deleting_an_automatic_name_keeps_its_saved_correction() {
    // Removing a mention edits the sentence, not the spelling of the name.
    // Cover both removing the only mention and leaving another corrected one.
    for (text, final_text) in [
        ("请找张山。", "请找。"),
        ("请找张山，张山在开会。", "请找，张珊在开会。"),
    ] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let raw = result("请找张山。", "张山", "同事");
        let prepared = memory.prepare(vec![raw.clone()], None);
        memory.confirm("first", &prepared, "请找张珊。", 100);
        let prepared = memory.prepare(vec![result(text, "张山", "同事")], None);
        memory.confirm("deleted", &prepared, final_text, 101);
        assert_eq!(memory.prepare(vec![raw], None).text, "请找张珊。");
    }
}

#[test]
fn punctuation_only_edits_do_not_retrain_automatic_names() {
    // Adding a full stop to a one-word preview must neither change the name nor
    // refresh its manual evidence; punctuation on one of two mentions must agree.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let mut memory = Memory::load(Some(path.clone()), Default::default(), 100).unwrap();
    let raw = result("Cody", "Cody", "项目");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("first", &prepared, "Codex", 100);
    memory.save().unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (index, (text, final_text)) in [
        ("Cody", "Codex."),
        ("Cody", "Codex！"),
        ("Cody; Cody", "Codex.; Codex"),
    ]
    .into_iter()
    .enumerate()
    {
        let prepared = memory.prepare(vec![result(text, "Cody", "项目")], None);
        memory.confirm(&format!("punctuation-{index}"), &prepared, final_text, 101);
        memory.save().unwrap();
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after["corrections"], saved["corrections"]);
        assert_eq!(memory.prepare(vec![raw.clone()], None).text, "Codex");
    }
}

#[test]
fn spelling_edits_do_not_include_added_sentence_punctuation() {
    // Correcting a name and ending the sentence together must teach the name
    // alone, including when changing a previously automatic spelling.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let raw = result("Cody", "Cody", "项目");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("first", &prepared, "Codex.", 100);
    assert_eq!(memory.prepare(vec![raw.clone()], None).text, "Codex");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("third-spelling", &prepared, "Codez。", 101);
    assert_eq!(memory.prepare(vec![raw], None).text, "Codez");
}

#[test]
fn automatic_preview_ranges_keep_their_original_names() {
    // Alpha becomes Beta while the original Beta becomes Gamma. Re-aligning raw
    // and displayed text by their shared Beta can assign feedback to the wrong name.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    for (index, (from, to)) in [("Alpha", "Beta"), ("Beta", "Gamma")]
        .into_iter()
        .enumerate()
    {
        let prepared = memory.prepare(vec![result(from, from, "项目")], None);
        memory.confirm(&format!("first-{index}"), &prepared, to, 100);
    }
    let mut raw = result("Alpha Beta", "Alpha", "项目");
    raw.memory_candidates.push(Candidate {
        kind: TermKind::ProperNoun,
        text: "Beta".into(),
    });
    let prepared = memory.prepare(vec![raw.clone()], None);
    assert_eq!(prepared.text, "Beta Gamma");
    memory.confirm("edited", &prepared, "Delta Gamma", 101);
    assert_eq!(memory.prepare(vec![raw], None).text, "Delta Gamma");
}

#[test]
fn spelling_feedback_preserves_name_symbols_and_existing_punctuation() {
    // Language names use symbols, and punctuation explicitly included by the
    // model belongs to its complete candidate rather than a new sentence ending.
    for (from, to) in [
        ("C plus plus", "C++"),
        ("DotNet", ".NET"),
        ("Acme Co.", "Example Co."),
    ] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let raw = result(from, from, "项目");
        let prepared = memory.prepare(vec![raw.clone()], None);
        memory.confirm("first", &prepared, to, 100);
        assert_eq!(memory.prepare(vec![raw], None).text, to);
    }
}

#[test]
fn inserted_and_deleted_letters_update_automatic_names() {
    // A third spelling can differ only by adding/removing letters. Name-edge
    // edits must work inside sentences and at the end of a multi-segment result.
    for name in ["Code", "Codexx", "XCodex", "Codx", "CoXdex"] {
        for (sources, shown, edited) in [
            (
                vec![result("Cody", "Cody", "项目")],
                "Codex",
                name.to_owned(),
            ),
            (
                vec![result("Use Cody.", "Cody", "项目")],
                "Use Codex.",
                format!("Use {name}."),
            ),
            (
                vec![
                    result("Ready.", "Ready", "项目"),
                    result("Use Cody", "Cody", "项目"),
                ],
                "Ready. Use Codex",
                format!("Ready. Use {name}"),
            ),
            (
                vec![
                    result("Use Cody", "Cody", "项目"),
                    result("Ready.", "Ready", "项目"),
                ],
                "Use Codex Ready.",
                format!("Use {name} Ready."),
            ),
        ] {
            let mut memory = Memory::load(None, Default::default(), 100).unwrap();
            let prepared = memory.prepare(vec![result("Cody", "Cody", "项目")], None);
            memory.confirm("first", &prepared, "Codex", 100);
            let prepared = memory.prepare(sources.clone(), None);
            assert_eq!(prepared.text, shown);
            memory.confirm("edited", &prepared, &edited, 101);
            assert_eq!(memory.prepare(sources, None).text, edited, "name: {name}");
        }
    }
}

#[test]
fn surrounding_inserted_text_does_not_extend_a_name() {
    // Permitting letter insertion must not teach a neighboring word or sentence
    // as part of the name merely because the name occupied the original segment.
    for edited in ["Our Codex", "Call Codex", "Codex team", "Codex.", "Codex！"] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let raw = result("Cody", "Cody", "项目");
        let prepared = memory.prepare(vec![raw.clone()], None);
        memory.confirm("first", &prepared, "Codex", 100);
        let prepared = memory.prepare(vec![raw.clone()], None);
        memory.confirm("surrounding-text", &prepared, edited, 101);
        assert_eq!(memory.prepare(vec![raw], None).text, "Codex");
    }
}

#[test]
fn complete_unchanged_names_anchor_their_final_spans() {
    // A neighboring word can share the name's first letters, making a grapheme
    // diff split the unchanged name and misinterpret the added word as a rename.
    for (name, edited) in [
        ("Codex", "Call Codex"),
        ("New York", "New New York"),
        ("C++", "Call C++"),
    ] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let raw = result(name, name, "项目");
        let prepared = memory.prepare(vec![raw.clone()], None);
        memory.confirm("surrounding-text", &prepared, edited, 100);
        let snapshot = memory.snapshot(100);
        assert!(snapshot.contexts[0].corrections.is_empty(), "name: {name}");
        assert!(
            snapshot.contexts[0]
                .candidates
                .iter()
                .any(|c| c.term.text == name)
        );
        assert_eq!(memory.prepare(vec![raw], None).text, name);
    }
}

#[test]
fn unseparated_chinese_letters_still_update_the_whole_name() {
    // Unicode word boundaries exist between Han characters. An old-name substring
    // without a separator must not hide a genuine prefix/suffix letter insertion.
    for name in ["张珊珊", "小张珊", "张小珊"] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let raw = result("张山", "张山", "同事");
        let prepared = memory.prepare(vec![raw.clone()], Some("zh".into()));
        memory.confirm("first", &prepared, "张珊", 100);
        let prepared = memory.prepare(vec![raw.clone()], Some("zh".into()));
        assert_eq!(prepared.text, "张珊");
        memory.confirm("edited", &prepared, name, 101);
        assert_eq!(memory.prepare(vec![raw], Some("zh".into())).text, name);
    }
}

#[test]
fn inserted_neighboring_words_cannot_rename_a_changed_name() {
    // Adding a neighboring word while editing the name leaves no intact anchor.
    // A word separator introduced by a local edit has ambiguous scope, including
    // when deleting a letter merges the following word into the same edit.
    for edited in ["Call Code", "Call Codexx", "Code Call"] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let raw = result("Codex", "Codex", "项目");
        let prepared = memory.prepare(vec![raw.clone()], None);
        memory.confirm("neighbor-and-edit", &prepared, edited, 100);
        assert!(memory.snapshot(100).contexts[0].corrections.is_empty());
        assert_eq!(memory.prepare(vec![raw], None).text, "Codex");
    }
    // An explicit whole-name rewrite can still introduce a multiword name.
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    let raw = result("维伯耳语", "维伯耳语", "项目");
    let prepared = memory.prepare(vec![raw.clone()], None);
    memory.confirm("whole-name", &prepared, "Viber Whisper", 100);
    assert_eq!(memory.prepare(vec![raw], None).text, "Viber Whisper");
}

#[test]
fn an_insertion_between_adjacent_names_has_no_unique_owner() {
    // With no separator, the inserted character could extend either name.
    // Neither candidate may claim it and train a guessed spelling.
    let mut raw = result("张山李四", "张山", "同事");
    raw.memory_candidates.push(Candidate {
        kind: TermKind::ProperNoun,
        text: "李四".into(),
    });
    for sources in [
        vec![raw],
        vec![
            result("张山", "张山", "同事"),
            result("李四", "李四", "朋友"),
        ],
    ] {
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        let prepared = memory.prepare(sources, Some("zh".into()));
        memory.confirm("ambiguous", &prepared, "张山小李四", 100);
        assert!(
            memory
                .snapshot(100)
                .contexts
                .iter()
                .all(|c| c.corrections.is_empty())
        );
    }
}

#[test]
fn unprojectable_neighbors_still_block_shared_edge_insertions() {
    // Deleting through a neighbor's boundary makes its projection fail. It still
    // owns its source edge, so the surviving name must not absorb the inserted 小.
    for (text, edited, pieces) in [
        (
            "张山李四。",
            "张山小李",
            [("张山", "张山"), ("李四", "李四"), ("。", "")],
        ),
        (
            "。张山李四",
            "山小李四",
            [("。", ""), ("张山", "张山"), ("李四", "李四")],
        ),
    ] {
        let mut raw = result(text, "张山", "同事");
        raw.memory_candidates.push(Candidate {
            kind: TermKind::ProperNoun,
            text: "李四".into(),
        });
        let segments = pieces
            .into_iter()
            .map(|(text, term)| result(text, term, "同事"))
            .collect();
        for sources in [vec![raw], segments] {
            let mut memory = Memory::load(None, Default::default(), 100).unwrap();
            let prepared = memory.prepare(sources, Some("zh".into()));
            memory.confirm("shared-edge", &prepared, edited, 100);
            assert!(
                memory
                    .snapshot(100)
                    .contexts
                    .iter()
                    .all(|c| c.corrections.is_empty()),
                "edited: {edited}"
            );
        }
    }
}
