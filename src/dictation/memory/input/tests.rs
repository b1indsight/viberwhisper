use crate::dictation::{
    config::MemoryConfig,
    memory::{Memory, Observation},
};
use serde_json::json;

#[test]
fn runtime_updates_and_expiry_do_not_change_the_declared_vocabulary() {
    // New segment history and yesterday's expiry used to change text before the audio.
    let settings: MemoryConfig = serde_json::from_value(json!({"long_term":[{
        "context":"project", "kind":"proper_noun", "text":"ViberWhisper"
    }]}))
    .unwrap();
    let memory = Memory::seeded(
        settings,
        vec![Observation::new(
            "previous",
            0,
            100,
            "project",
            "使用 ViberWhisper",
            Vec::new(),
        )],
        100,
    );
    let stable = memory.stable_prefix();
    let before = memory.prompt_input(&stable, 100);
    let expired = memory.prompt_input(&stable, 86_500);
    assert_eq!(
        serde_json::to_string(&before.stable).unwrap(),
        r#"{"contexts":[{"id":"project","label":"project","long_term":[{"context":"project","kind":"proper_noun","text":"ViberWhisper"}]}]}"#
    );
    assert_eq!(before.stable, expired.stable);
    assert_eq!(stable, memory.stable_prefix());
    assert_eq!(before.dynamic.contexts[0].history, ["使用 ViberWhisper"]);
    assert!(before.dynamic.contexts[0].long_term.is_empty());
    assert!(expired.dynamic.contexts.is_empty());
    assert_eq!(before.snapshot(), memory.snapshot(100));
    assert_eq!(expired.snapshot(), memory.snapshot(86_500));
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
