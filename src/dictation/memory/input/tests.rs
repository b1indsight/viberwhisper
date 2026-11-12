use crate::dictation::{
    config::MemoryConfig,
    memory::{Memory, Observation},
};
use serde_json::json;

#[test]
fn natural_reference_preserves_terms_corrections_and_history_without_program_fields() {
    // Changing model-facing representation must not drop corrected spelling, duplicate
    // history or topic boundaries, or expose candidate support metadata as model input.
    let snapshot: super::Snapshot = serde_json::from_value(json!({"contexts":[{
        "id":"项目", "label":"项目", "long_term":[],
        "candidates":[{"context":"项目","kind":"proper_noun","text":"C++ Tools","supporting_recordings":7}],
        "corrections":[{"from":"维伯耳语","to":"ViberWhisper"}],
        "history":["新句，保留\"引号\"。\n下一行", "旧句", "旧句"]
    }, {
        "id":"另一话题", "label":"另一话题", "long_term":[], "candidates":[], "history":[]
    }]})).unwrap();
    let text = snapshot.reference_text();
    assert!(text.contains("历史话题：项目。\n候选参考词：C++ Tools。"));
    assert!(text.contains("将“维伯耳语”写作“ViberWhisper”"));
    assert!(text.contains("1. 新句，保留\"引号\"。\n下一行\n2. 旧句\n3. 旧句"));
    assert!(text.contains("历史话题：另一话题。\n候选参考词：无。"));
    assert!(!text.contains("supporting_recordings") && !text.contains("proper_noun"));
    assert!(serde_json::from_str::<serde_json::Value>(&text).is_err());
    let settings: MemoryConfig = serde_json::from_value(json!({"long_term":[{
        "context":"人工话题", "kind":"proper_noun", "text":"Orion Pro"
    }]}))
    .unwrap();
    let prefix = Memory::frozen(settings).stable_prefix().reference_text();
    assert!(prefix.contains("话题：人工话题。\n人工参考词：Orion Pro。"));
    assert!(!prefix.contains("旧句"));
    assert_eq!(
        Memory::frozen(MemoryConfig::default())
            .stable_prefix()
            .reference_text(),
        "人工参考词：暂无。"
    );
    assert!(
        super::Snapshot::default()
            .reference_text()
            .ends_with("暂无历史参考资料。")
    );
}

#[test]
fn dynamic_context_cannot_evict_a_frozen_prefix_at_the_joint_budget_limit() {
    // A later topic adds JSON envelope overhead even after all its history is trimmed.
    // Reserve the accepted vocabulary and remove dynamic-only topics before altering it.
    let terms: Vec<_> = (0..3)
        .flat_map(|context| {
            (0..16).map(move |i| {
                json!({
                    "context":format!("topic-{context}"), "kind":"proper_noun",
                    "text":format!("{i:02}{}", "词".repeat(78))
                })
            })
        })
        .collect();
    let settings: MemoryConfig = serde_json::from_value(json!({"long_term":terms})).unwrap();
    let memory = Memory::seeded(
        settings,
        vec![Observation::new(
            "previous",
            0,
            100,
            "dynamic",
            &"历史".repeat(512),
            Vec::new(),
        )],
        100,
    );
    let stable = memory.stable_prefix();
    let input = memory.prompt_input(&stable, 100);
    let merged = input.snapshot();
    assert_eq!(merged, memory.snapshot(100));
    assert!(merged.within_budget());
    assert!(merged.contexts.len() <= 4);
    assert_eq!(input.stable, stable);
    for context in stable.contexts {
        assert_eq!(
            merged
                .contexts
                .iter()
                .find(|c| c.id == context.id)
                .unwrap()
                .long_term,
            context.long_term
        );
    }
}
