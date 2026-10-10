use super::*;
use crate::dictation::test_support::{
    accept_session, commit_audio, delete_audio, expect_event, listen, send_json,
};
use crate::{
    core::orchestrator::{SessionOrchestrator, SessionRoutingError},
    session::SessionId,
};
use serde_json::json;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn disconnect_after_completed_response_preserves_only_validated_text() {
    for valid in [true, false] {
        let (listener, settings) = listen().await;
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            commit_audio(&mut socket, "audio").await;
            let event = expect_event(&mut socket, "response.create").await;
            let text = if valid {
                json!({"text":"已完成。", "context":{"kind":"new","label":"工作"}, "memory_candidates":[{"kind":"phrase","text":"已完成"}]}).to_string()
            } else {
                "not JSON".into()
            };
            for reply in [
                json!({"type":"response.created","response":{"id":"r","metadata":event["response"]["metadata"]}}),
                json!({"type":"response.output_text.done","response_id":"r","output_index":0,"content_index":0,"text":text}),
                json!({"type":"response.done","response":{"id":"r","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}),
            ] {
                send_json(&mut socket, reply).await;
            }
            // The service disconnects after completing transcription, before cleanup.
            socket.close(None).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let engine =
            SessionOrchestrator::new_realtime(settings, None, Some(path.clone()), None).unwrap();
        let error = tokio::task::spawn_blocking(move || {
            let id = SessionId(1);
            engine.start_session(id).unwrap();
            engine.on_chunk_ready(
                id,
                WavChunk::from_encoded_bytes(
                    include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
                ),
            );
            engine.finish_session(id).unwrap_err()
        })
        .await
        .unwrap();
        let SessionError::PartialFailure { partial_text, .. } = error else {
            panic!("expected cleanup/validation failure")
        };
        assert_eq!(partial_text, if valid { "已完成。" } else { "" });
        assert!(!path.exists(), "cleanup failure must not commit memory");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn completed_segment_survives_faults_but_not_user_cancellation() {
    #[derive(Clone, Copy, Debug)]
    enum Stop {
        QueueOverflow,
        RecorderFailure,
        UserCancellation,
    }
    for stop in [
        Stop::QueueOverflow,
        Stop::RecorderFailure,
        Stop::UserCancellation,
    ] {
        let (listener, settings) = listen().await;
        let (waiting, blocked) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            commit_audio(&mut socket, "audio-1").await;
            let event = expect_event(&mut socket, "response.create").await;
            let text = json!({"text":"保留这一段。", "context":{"kind":"new","label":"工作"}, "memory_candidates":[{"kind":"phrase","text":"保留这一段"}]}).to_string();
            for reply in [
                json!({"type":"response.created","response":{"id":"r1","metadata":event["response"]["metadata"]}}),
                json!({"type":"response.output_text.done","response_id":"r1","output_index":0,"content_index":0,"text":text}),
                json!({"type":"response.done","response":{"id":"r1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}),
            ] {
                send_json(&mut socket, reply).await;
            }
            delete_audio(&mut socket, "audio-1").await;
            expect_event(&mut socket, "input_audio_buffer.commit").await;
            // Hold the second commit after the first segment has been learned locally.
            // This makes backlog overflow deterministic and tests rollback of that memory.
            waiting.send(()).unwrap();
            resume.await.unwrap();
            send_json(
                &mut socket,
                json!({"type":"input_audio_buffer.committed","item_id":"audio-2"}),
            )
            .await;
            delete_audio(&mut socket, "audio-2").await;
            expect_event(&mut socket, "input_audio_buffer.clear").await;
            send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let engine =
            SessionOrchestrator::new_realtime(settings, None, Some(path.clone()), None).unwrap();
        let id = SessionId(9);
        engine.start_session(id).unwrap();
        // Feed the 3-second fixture until two 30-second commits are reached.
        let frame = WavChunk::from_encoded_bytes(
            include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        for _ in 0..21 {
            engine.on_chunk_ready(id, frame.clone());
        }
        timeout(Duration::from_secs(15), blocked)
            .await
            .unwrap()
            .unwrap();
        match stop {
            Stop::QueueOverflow => {
                let frame = crate::audio::chunk::encode_i16_wav(&[1; 3200], 16_000).unwrap();
                for _ in 0..=FRAME_QUEUE {
                    engine.on_chunk_ready(id, frame.clone());
                }
                assert!(engine.recording_error(id).unwrap().contains("capacity"));
            }
            Stop::RecorderFailure => engine.fail_session(id, "recorder overflow"),
            Stop::UserCancellation => engine.abort_session(id).unwrap(),
        }
        release.send(()).unwrap();
        let error = timeout(
            Duration::from_secs(15),
            tokio::task::spawn_blocking(move || engine.finish_session(id).unwrap_err()),
        )
        .await
        .unwrap()
        .unwrap();
        if matches!(stop, Stop::UserCancellation) {
            // Cancellation detaches the session: no result can reach delivery afterwards.
            assert!(matches!(
                error,
                SessionError::Routing(SessionRoutingError::NoActiveSession { .. })
            ));
        } else {
            let SessionError::PartialFailure { partial_text, .. } = error else {
                panic!("expected partial failure");
            };
            assert_eq!(partial_text, "保留这一段。", "{stop:?}");
        }
        server.await.unwrap();
        assert!(
            !path.exists(),
            "failed or cancelled recording must not learn"
        );
    }
}
