use super::{
    UNKNOWN_CONTEXT,
    config::{DictationConfig, MemoryConfig},
    memory::Memory,
    result::DictationResult,
};
use serde_json::json;

fn response(text: &str) -> String {
    json!({"text": text,
        "context": "Rust 项目",
        "memory_candidates":["ViberWhisper"]
    })
    .to_string()
}

#[test]
fn string_metadata_preserves_repeated_names_in_source_order() {
    // The simplified protocol must keep every supplied mention, not deduplicate a name.
    let wire = json!({
        "context": "开发",
        "memory_candidates": ["Codex", "Redis", "Codex"],
        "text": "用 Codex 连接 Redis，再让 Codex 检查。"
    });
    let result = DictationResult::parse(&wire.to_string()).unwrap();
    assert_eq!(result.context.as_deref(), Some("开发"));
    assert_eq!(
        result
            .memory_candidates
            .iter()
            .map(|candidate| (
                candidate.text.as_str(),
                &result.text[candidate.span.clone()]
            ))
            .collect::<Vec<_>>(),
        [("Codex", "Codex"), ("Redis", "Redis"), ("Codex", "Codex")]
    );
}

#[test]
fn realtime_results_keep_segment_text_and_json_limits() {
    // Aggregate HTTP cleanup must not relax the bounds used for Realtime segments.
    for length in [16_384, 16_385] {
        let wire = json!({"text":"a".repeat(length)}).to_string();
        assert_eq!(DictationResult::parse(&wire).is_ok(), length == 16_384);
        assert!(DictationResult::parse_transcript(&wire).is_ok());
    }
    let mut padded = r#"{"text":"ok"}"#.to_owned();
    padded.push_str(&" ".repeat(64 * 1024 - padded.len()));
    assert!(DictationResult::parse(&padded).is_ok());
    padded.push(' ');
    assert!(DictationResult::parse(&padded).is_err());
    for wire in [
        "```json\n{}\n```",
        "{}",
        r#"{"text":3}"#,
        r#"{"text":"\u0000"}"#,
    ] {
        assert!(DictationResult::parse(wire).is_err());
    }
}

#[test]
fn invalid_context_preserves_text_without_learning() {
    let mut wires: Vec<_> = [
        json!(null),
        json!(42),
        json!({"kind":"new", "label":"项目"}),
        json!(""),
        json!("话题".repeat(33)),
        json!("项目\n"),
    ]
    .into_iter()
    .map(|context| {
        json!({
            "context":context, "memory_candidates":["ViberWhisper"], "text":"用ViberWhisper。"
        })
    })
    .collect();
    wires.push(json!({"memory_candidates":["ViberWhisper"], "text":"用ViberWhisper。"}));
    for wire in wires {
        let result = DictationResult::parse(&wire.to_string()).unwrap();
        assert_eq!(result.text, "用ViberWhisper。");
        assert!(result.context.is_none());
        assert!(result.memory_candidates.is_empty());
        let mut memory = Memory::load(None, Default::default(), 100).unwrap();
        memory.learn("raw", 0, &result, 100);
        let prepared = memory.prepare(vec![result], None);
        memory.confirm("confirmed", &prepared, "用ViberWhispr。", 100);
        assert!(memory.snapshot(100).contexts.is_empty());
    }
}

#[test]
fn candidate_groups_require_unambiguous_complete_occurrences() {
    // Counts must distinguish all mentions from a selected subset; full names
    // own their fragments, and crossing names must not choose each other's spans.
    for (text, names, expected) in [
        ("请找张山。", vec!["张山"], vec!["张山"]),
        ("张山和张山。", vec!["张山"], vec![]),
        ("张山和张山。", vec!["张山", "张山"], vec!["张山", "张山"]),
        ("张山和张山。", vec!["张山", "张山", "张山"], vec![]),
        ("哈哈哈 Redis", vec!["哈哈", "Redis"], vec!["Redis"]),
        (
            "New York; York",
            vec!["York", "New York"],
            vec!["New York", "York"],
        ),
        (
            "New York; Redis",
            vec!["New York", "New York", "York", "Redis"],
            vec!["Redis"],
        ),
        (
            "New York City; Redis",
            vec!["New York", "York City", "Redis"],
            vec!["Redis"],
        ),
        (
            "New York City; Redis",
            vec!["New York City", "New York", "York City", "Redis"],
            vec!["New York City", "Redis"],
        ),
        (
            "Use Codycode and Redis.",
            vec!["Cody", "Redis"],
            vec!["Redis"],
        ),
    ] {
        let wire = json!({"text":text,"context":"同事", "memory_candidates":names});
        let result = DictationResult::parse(&wire.to_string()).unwrap();
        assert_eq!(result.text, text);
        assert_eq!(
            result
                .memory_candidates
                .iter()
                .map(|c| &result.text[c.span.clone()])
                .collect::<Vec<_>>(),
            expected,
            "wire: {wire}"
        );
    }
}

#[test]
fn candidate_limits_drop_invalid_metadata_without_losing_valid_neighbors() {
    let names = [
        "Codex", "Redis", "Rust", "Python", "GitHub", "Windows", "macOS", "Docker", "SQLite",
    ];
    for (count, expected) in [(8, 8), (9, 0)] {
        let wire =
            json!({"text":names.join(" "), "context":"开发", "memory_candidates":&names[..count]});
        let result = DictationResult::parse(&wire.to_string()).unwrap();
        assert_eq!(result.memory_candidates.len(), expected);
        assert_eq!(result.context.as_deref(), Some("开发"));
    }
    for (length, expected) in [(80, 2), (81, 1)] {
        let name = "名".repeat(length);
        let wire = json!({"text":format!("{name} Redis"), "context":"开发", "memory_candidates":[name, "Redis"]});
        let result = DictationResult::parse(&wire.to_string()).unwrap();
        assert_eq!(result.memory_candidates.len(), expected);
        assert_eq!(result.memory_candidates.last().unwrap().text, "Redis");
    }
    for metadata in [json!(null), json!("Redis"), json!({"text":"Redis"})] {
        let wire = json!({"text":"Redis", "context":"开发", "memory_candidates":metadata});
        let result = DictationResult::parse(&wire.to_string()).unwrap();
        assert_eq!(result.text, "Redis");
        assert_eq!(result.context.as_deref(), Some("开发"));
        assert!(result.memory_candidates.is_empty());
    }
}

#[test]
fn candidates_absent_from_current_text_cannot_pollute_learned_memory() {
    // A real response invented a candidate absent even from its own transcript.
    // Keep the transcript and grounded names, including harmless case/full-width differences.
    let wire = json!({"text":"使用ｒｅｄｉｓ缓存。", "context":"开发",
        "memory_candidates":["Redis", "3核38都是无统的", 42, {"text":"Redis"}]});
    let result = DictationResult::parse(&wire.to_string()).unwrap();
    assert_eq!(result.text, "使用ｒｅｄｉｓ缓存。");
    assert_eq!(result.memory_candidates.len(), 1);
    assert_eq!(result.memory_candidates[0].text, "Redis");
    assert_eq!(result.raw["memory_candidates"].as_array().unwrap().len(), 4);
    let mut memory = Memory::load(None, Default::default(), 100).unwrap();
    memory.learn("recording", 0, &result, 100);
    let snapshot = serde_json::to_value(memory.snapshot(100)).unwrap();
    assert_eq!(
        snapshot["contexts"][0]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(snapshot["contexts"][0]["candidates"][0]["text"], "Redis");
}

#[test]
fn short_term_memory_expires_live_and_after_restart_but_long_term_survives() {
    for configured in [false, true] {
        let settings: DictationConfig =
            serde_json::from_value(json!({"memory":{"long_term":if configured {
            json!([{"context":"Rust 项目","kind":"proper_noun","text":"ViberWhisper"}])
        } else { json!([]) }}}))
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let mut memory = Memory::load(Some(path.clone()), settings.memory.clone(), 100).unwrap();
        let result = DictationResult::parse(&response("ViberWhisper。")).unwrap();
        memory.learn("once", 0, &result, 100);
        assert!(!memory.snapshot(100 + 86_399).ids().is_empty());
        memory.save().unwrap();
        // No reload or subsequent learn/prune call: a running worker must expire it too.
        let expired = memory.snapshot(100 + 86_400);
        assert!(expired.history_is_empty());
        assert_eq!(expired.ids().contains(&"Rust 项目".into()), configured);
        let reloaded = Memory::load(Some(path), settings.memory, 100 + 86_400).unwrap();
        assert_eq!(reloaded.snapshot(100 + 86_400), expired);
    }
}

#[test]
fn unknown_context_cannot_apply_or_learn_topic_corrections() {
    let mut memory = Memory::load(None, Default::default(), 0).unwrap();
    let source = DictationResult::parse(&response("ViberWhisper。")).unwrap();
    let prepared = memory.prepare(vec![source], None);
    memory.confirm("known", &prepared, "ViberWhispr。", 1);
    let before = memory.snapshot(2);
    for candidates in [json!([]), json!(["ViberWhisper"]), json!(null)] {
        let wire = json!({"context":UNKNOWN_CONTEXT, "memory_candidates":candidates, "text":"ViberWhisper。"});
        let result = DictationResult::parse(&wire.to_string()).unwrap();
        assert!(result.context.is_none());
        assert!(result.memory_candidates.is_empty());
        assert_eq!(result.raw["context"], UNKNOWN_CONTEXT);
        memory.learn("unknown", 0, &result, 2);
        let prepared = memory.prepare(vec![result], None);
        assert_eq!(prepared.text, "ViberWhisper。");
        memory.confirm("unknown-confirmed", &prepared, "ViberWhispar。", 2);
        assert_eq!(memory.snapshot(2), before);
    }
}

#[test]
fn reserved_context_cannot_evict_real_topics_or_rewrite_legacy_storage() {
    // A legacy reserved-name correction comes first; filtering after take(4)
    // would hide one of the four real topics even though the file is still valid.
    assert!(super::config::valid_label(UNKNOWN_CONTEXT));
    let settings: MemoryConfig = serde_json::from_value(json!({"long_term":[{
        "context":UNKNOWN_CONTEXT, "kind":"proper_noun", "text":"Codex"
    }]}))
    .unwrap();
    let error = settings.validate().unwrap_err().to_string();
    assert!(error.contains("reserved") && error.contains("rename"));

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let observations: Vec<_> = ["one", "two", "three", "four", UNKNOWN_CONTEXT]
        .into_iter()
        .map(|context| {
            json!({
                "recording":format!("legacy-{context}"), "segment":0, "at":100,
                "context":context, "text":"既有短语", "terms":if context == "one" {
                    json!([{"context":"one", "kind":"phrase", "text":"既有短语"}])
                } else { json!([]) }
            })
        })
        .collect();
    let bytes = serde_json::to_vec(&json!({
        "schema_version":2, "observations":observations,
        "corrections":[{"context":UNKNOWN_CONTEXT, "rule":{"from":"Cody", "to":"Codex"}, "recording":"legacy-rule", "at":100}]
    })).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let memory = Memory::load(Some(path.clone()), Default::default(), 100).unwrap();
    let snapshot = memory.snapshot(100);
    assert_eq!(snapshot.ids(), ["four", "three", "two", "one"]);
    assert_eq!(
        serde_json::to_value(snapshot).unwrap()["contexts"][3]["candidates"][0]["kind"],
        "phrase"
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn repeated_terms_count_recordings_and_refresh_only_from_new_observations() {
    // The second response refers to the snapshot's existing context and vocabulary.
    let mut memory = Memory::load(None, Default::default(), 0).unwrap();
    let first = DictationResult::parse(&response("ViberWhisper。")).unwrap();
    memory.learn("first", 0, &first, 1);
    let once = memory.snapshot(1);
    memory.learn("first", 0, &first, 2);
    assert_eq!(
        memory.snapshot(2),
        once,
        "a replay must not create an observation"
    );
    memory.learn("first", 1, &first, 2);
    let snapshot = memory.snapshot(80_000);
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap()["contexts"][0]["candidates"][0]["supporting_recordings"],
        1
    );
    let mut wire: serde_json::Value = serde_json::from_str(&response("ViberWhisper。")).unwrap();
    wire["context"] = json!(snapshot.ids()[0]);
    let again = DictationResult::parse(&wire.to_string()).unwrap();
    memory.learn("second", 0, &again, 80_000);
    assert_eq!(
        serde_json::to_value(memory.snapshot(80_000)).unwrap()["contexts"][0]["candidates"][0]["supporting_recordings"],
        2
    );
    assert!(
        serde_json::to_string(&memory.snapshot(90_000))
            .unwrap()
            .contains("ViberWhisper")
    );
}

#[test]
fn manual_vocabulary_keeps_context_and_budget_before_recent_history() {
    let settings: DictationConfig = serde_json::from_value(json!({"memory":{"long_term":[
        {"context":"manual", "kind":"proper_noun", "text":"ViberWhisper"},
        {"context":"second", "kind":"proper_noun", "text":"Codex"}
    ]}}))
    .unwrap();
    let mut memory = Memory::load(None, settings.memory, 0).unwrap();
    for i in 0..8 {
        let text = "大量历史".repeat(250);
        let mut wire: serde_json::Value = serde_json::from_str(&response(&text)).unwrap();
        wire["context"] = json!(format!("topic-{i}"));
        memory.learn(
            &format!("record-{i}"),
            0,
            &DictationResult::parse(&wire.to_string()).unwrap(),
            i,
        );
    }
    let snapshot = memory.snapshot(9);
    assert!(snapshot.ids().contains(&"manual".to_string()));
    assert!(snapshot.ids().contains(&"second".to_string()));
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(serialized.contains("ViberWhisper") && serialized.contains("Codex"));
    assert!(serialized.chars().count() <= 6000);
}

#[test]
fn manual_terms_survive_other_contexts_large_history() {
    // Two configured contexts fit in the prompt together; earlier history must
    // be discarded globally before vocabulary from the later context is cut.
    let terms: Vec<_> = ["first", "second"].into_iter().flat_map(|context| {
        (0..16).map(move |i| json!({"context":context,"kind":"proper_noun","text":format!("{i:02}{}", "词".repeat(78))}))
    }).collect();
    let settings: DictationConfig =
        serde_json::from_value(json!({"memory":{"long_term":terms}})).unwrap();
    let mut memory = Memory::load(None, settings.memory, 0).unwrap();
    for i in 0..6 {
        let text = "历史".repeat(500);
        let wire = json!({"text":text,"context":if i < 3 {"first"} else {"second"},"memory_candidates":[]});
        memory.learn(
            &format!("record-{i}"),
            0,
            &DictationResult::parse(&wire.to_string()).unwrap(),
            i,
        );
    }
    let snapshot = memory.snapshot(7);
    assert_eq!(
        snapshot
            .contexts
            .iter()
            .map(|c| c.long_term.len())
            .sum::<usize>(),
        32
    );
    assert!(serde_json::to_string(&snapshot).unwrap().chars().count() <= 6000);
}

#[test]
fn realtime_settings_ignore_unused_http_and_cleanup_settings() {
    let mut document: crate::core::config::ConfigDocument =
        serde_json::from_str(include_str!("../../config.example.json")).unwrap();
    document.dictation.enabled = true;
    document.inference.api.transcription.api_url = "not a URL".into();
    document.post_process.enabled = true;
    document.inference.api.post_process.api_url = None;
    let config = crate::core::listener::ListenerConfig::from_config(&document).unwrap();
    assert!(config.recording.recognition.is_realtime());
    assert!(matches!(
        config.post_process,
        crate::postprocess::PostProcessConfig::Disabled
    ));
}
