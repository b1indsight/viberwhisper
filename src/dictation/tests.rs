use super::{config::DictationConfig, memory::Memory, result::DictationResult};
use serde_json::json;

fn response(text: &str) -> String {
    json!({"text": text,
        "context": {"kind":"new", "label":"Rust 项目"},
        "memory_candidates":[{"kind":"proper_noun", "text":"ViberWhisper"}]
    })
    .to_string()
}

#[test]
fn invalid_memory_does_not_discard_text_and_cannot_invent_context() {
    assert!(DictationResult::parse("```json\n{}\n```", &[]).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&response("你好。")).unwrap();
    value["context"] = json!({"kind":"existing", "id":"not-in-snapshot"});
    let result = DictationResult::parse(&value.to_string(), &[]).unwrap();
    assert_eq!(result.text, "你好。");
    assert!(result.context.is_none());
    assert!(result.memory_candidates.is_empty());
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
        let result = DictationResult::parse(&response("ViberWhisper。"), &[]).unwrap();
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
fn uncertain_topics_are_not_learned() {
    let mut value: serde_json::Value = serde_json::from_str(&response("你好。")).unwrap();
    value["context"] = json!({"kind":"uncertain"});
    let result = DictationResult::parse(&value.to_string(), &[]).unwrap();
    let mut memory = Memory::load(None, Default::default(), 0).unwrap();
    memory.learn("recording-b", 0, &result, 1);
    assert!(memory.snapshot(1).history_is_empty());
}

#[test]
fn repeated_terms_count_recordings_and_refresh_only_from_new_observations() {
    // The second response refers to the snapshot's existing context and vocabulary.
    let mut memory = Memory::load(None, Default::default(), 0).unwrap();
    let first = DictationResult::parse(&response("ViberWhisper。"), &[]).unwrap();
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
    wire["context"] = json!({"kind":"existing", "id":snapshot.ids()[0]});
    let again = DictationResult::parse(&wire.to_string(), &snapshot.ids()).unwrap();
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
        wire["context"] = json!({"kind":"new","label":format!("topic-{i}")});
        memory.learn(
            &format!("record-{i}"),
            0,
            &DictationResult::parse(&wire.to_string(), &[]).unwrap(),
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
        let wire = json!({"text":text,"context":{"kind":"new","label":if i < 3 {"first"} else {"second"}},"memory_candidates":[]});
        memory.learn(
            &format!("record-{i}"),
            0,
            &DictationResult::parse(&wire.to_string(), &[]).unwrap(),
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
