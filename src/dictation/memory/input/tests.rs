use crate::dictation::{
    config::MemoryConfig,
    memory::{Memory, Observation},
};
use serde_json::json;

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
