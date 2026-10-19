use super::*;
use crate::dictation::test_support::{
    accept_session, commit_segment, delete_segment, expect_event, listen, send_json,
};
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
        prompt_input: None,
        index: 0,
        input: state.snapshot(100),
        output: json!({
            "text":"ViberWhisper ready", "context":{"kind":"existing","id":"project"},
            "memory_candidates":[{"kind":"proper_noun","text":"ViberWhisper","quote":"ViberWhisper ready"}]
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
    let result =
        super::super::result::DictationResult::parse(&trace.output.to_string(), &trace.input.ids())
            .unwrap();
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
        prompt_input: None,
        input: snapshot,
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
    let mut tampered = trace;
    tampered.input.contexts.clear();
    assert!(state.replay("first", 100, &[tampered], true).is_err());
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

#[tokio::test]
async fn evaluator_sends_production_snapshot_and_rolls_back_cleanup_failure() {
    for cleanup in [true, false] {
        let (listener, mut settings) = listen().await;
        settings.prompt = Some("test instructions".into());
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            let declared = commit_segment(&mut socket, "audio").await;
            let event = expect_event(&mut socket, "response.create").await;
            assert_eq!(event["response"]["instructions"], "test instructions");
            let snapshot: Snapshot = serde_json::from_str(
                event["response"]["input"][2]["content"][0]["text"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            let stable: Value =
                serde_json::from_str(declared["item"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(
                stable["contexts"][0]["long_term"],
                serde_json::to_value(seeded().long_term).unwrap()
            );
            assert!(
                snapshot.contexts.is_empty(),
                "vocabulary must not be repeated after audio"
            );
            let text = json!({"text":"ViberWhisper ready","context":{"kind":"existing","id":"project"},"memory_candidates":[{"kind":"proper_noun","text":"ViberWhisper","quote":"ViberWhisper ready"}]}).to_string();
            for reply in [
                json!({"type":"response.created","response":{"id":"r","metadata":event["response"]["metadata"]}}),
                json!({"type":"response.output_text.done","response_id":"r","output_index":0,"content_index":0,"text":text}),
                json!({"type":"response.done","response":{"id":"r","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}),
            ] {
                send_json(&mut socket, reply).await;
            }
            if cleanup {
                delete_segment(&mut socket, "audio").await;
            } else {
                socket.close(None).await.unwrap();
            }
        });
        tokio::task::spawn_blocking(move || {
            let mut evaluator = RealtimeEvaluator::new(settings, None, &seeded(), 100).unwrap();
            let before = evaluator.snapshot(100);
            let (result, traces) = evaluator.transcribe(
                Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/audio/speech.wav"
                )),
                "first",
                100,
                None,
            );
            assert_eq!(result.is_ok(), cleanup);
            assert_eq!(traces.len(), 1);
            assert_eq!(traces[0].input, before);
            let replay = Replay::new(&seeded(), 100)
                .replay("first", 100, &traces, cleanup)
                .unwrap();
            assert_eq!(evaluator.snapshot(100), replay.snapshot(100));
            assert_eq!(
                evaluator.snapshot(100).contexts[0].history.is_empty(),
                !cleanup
            );
        })
        .await
        .unwrap();
        server.await.unwrap();
    }
}
