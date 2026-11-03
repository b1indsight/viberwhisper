use super::*;
use crate::dictation::test_support::{
    accept_session, commit_segment, complete_response, declare_prefix, delete_segment,
    expect_event, listen, send_json,
};
use crate::{
    core::orchestrator::{SessionOrchestrator, SessionRoutingError},
    session::SessionId,
};
use serde_json::json;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn failed_recording_reconnects_with_a_complete_prefix_and_fresh_audio() {
    // A dropped connection invalidates both item types; the next user recording
    // must declare its entire prefix again rather than replay the interrupted upload.
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        let first = declare_prefix(&mut socket, "retired-prefix").await;
        expect_event(&mut socket, "input_audio_buffer.append").await;
        socket.close(None).await.unwrap();
        let mut socket = accept_session(&listener).await;
        let next = commit_segment(&mut socket, "new-audio").await;
        assert_eq!(first["item"]["content"], next["item"]["content"]);
        let request = expect_event(&mut socket, "response.create").await;
        assert_eq!(request["response"]["input"][0]["id"], "prefix-new-audio");
        assert_eq!(request["response"]["input"][1]["id"], "new-audio");
        complete_response(
            &mut socket,
            &request,
            "new-response",
            r#"{"memory_candidates":[],"context":"未知语境","text":"重试成功"}"#,
        )
        .await;
        delete_segment(&mut socket, "new-audio").await;
    });
    timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            let engine = SessionOrchestrator::new_realtime(settings, None, None, None).unwrap();
            for turn in 0..2 {
                let id = SessionId(turn);
                engine.start_session(id).unwrap();
                engine.on_chunk_ready(
                    id,
                    WavChunk::from_encoded_bytes(
                        include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
                    ),
                );
                let result = engine.finish_for_delivery(id);
                if turn == 0 {
                    assert!(result.is_err());
                } else {
                    assert_eq!(result.unwrap().text, "重试成功");
                }
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn silence_and_empty_recordings_leave_no_prefix_for_the_next_recording() {
    // Silence has no committed audio item, but still owns a declared text item.
    // Clearing only audio used to be sufficient; now text must be deleted as well.
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        declare_prefix(&mut socket, "silent-prefix").await;
        expect_event(&mut socket, "input_audio_buffer.clear").await;
        send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
        crate::dictation::test_support::delete_audio(&mut socket, "silent-prefix").await;
        commit_segment(&mut socket, "voiced").await;
        let request = expect_event(&mut socket, "response.create").await;
        assert_eq!(request["response"]["input"][0]["id"], "prefix-voiced");
        complete_response(
            &mut socket,
            &request,
            "voiced-response",
            r#"{"memory_candidates":[],"context":"未知语境","text":"有声音"}"#,
        )
        .await;
        delete_segment(&mut socket, "voiced").await;
    });
    timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            let engine = SessionOrchestrator::new_realtime(settings, None, None, None).unwrap();
            for turn in 0..3 {
                let id = SessionId(turn);
                engine.start_session(id).unwrap();
                if turn == 0 {
                    engine.on_chunk_ready(
                        id,
                        crate::audio::chunk::encode_i16_wav(&[0; 4800], 24000).unwrap(),
                    );
                } else if turn == 2 {
                    engine.on_chunk_ready(
                        id,
                        WavChunk::from_encoded_bytes(
                            include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
                        ),
                    );
                }
                let text = engine.finish_for_delivery(id).unwrap().text;
                assert_eq!(text, if turn == 2 { "有声音" } else { "" });
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn feedback_commits_only_confirmed_text_before_the_next_recording() {
    // Closing a preview discards its model observations. An edited confirmation
    // must be visible in the next actual request, with no race against worker jobs.
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        for turn in 0..3 {
            let audio = format!("audio-{turn}");
            commit_segment(&mut socket, &audio).await;
            let event = expect_event(&mut socket, "response.create").await;
            let snapshot: serde_json::Value = serde_json::from_str(
                event["response"]["input"][2]["content"][0]["text"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            if turn < 2 {
                assert_eq!(snapshot["contexts"], json!([]));
            } else {
                assert_eq!(snapshot["contexts"][0]["history"][0], "用ViberWhisper。");
                assert_eq!(
                    snapshot["contexts"][0]["corrections"][0]["to"],
                    "ViberWhisper"
                );
            }
            let text = json!({"text":"用维伯耳语。", "context":"项目",
                "memory_candidates":["维伯耳语"]})
            .to_string();
            complete_response(&mut socket, &event, &format!("r-{turn}"), &text).await;
            delete_segment(&mut socket, &audio).await;
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let engine =
        SessionOrchestrator::new_realtime(settings, None, Some(path.clone()), Some("zh".into()))
            .unwrap();
    timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            for turn in 0..3 {
                let id = SessionId(turn);
                engine.start_session(id).unwrap();
                engine.on_chunk_ready(
                    id,
                    WavChunk::from_encoded_bytes(
                        include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
                    ),
                );
                let transcript = engine.finish_for_delivery(id).unwrap();
                if turn == 0 {
                    assert!(!path.exists(), "unconfirmed memory must not be saved");
                    drop(transcript);
                } else if turn == 1 {
                    assert!(
                        !path.exists(),
                        "cancelled preview must not train the next recording"
                    );
                    transcript
                        .confirmation
                        .unwrap()
                        .confirm("用ViberWhisper。".into())
                        .unwrap();
                } else {
                    assert_eq!(transcript.text, "用ViberWhisper。");
                    // The next job starts only after the prior feedback is committed.
                    let file: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    assert_eq!(file["observations"][0]["text"], "用ViberWhisper。");
                    drop(transcript);
                }
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn shutdown_drains_confirmation_without_another_recording() {
    // The listener can exit while a delivery thread still owns the orchestrator.
    // Explicit shutdown must flush accepted feedback and close an unconfirmed preview.
    for confirmed in [true, false] {
        let (listener, settings) = listen().await;
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            commit_segment(&mut socket, "audio").await;
            let event = expect_event(&mut socket, "response.create").await;
            let text = json!({"text":"Use Cody.", "context":"项目",
                "memory_candidates":["Cody"]})
            .to_string();
            complete_response(&mut socket, &event, "r", &text).await;
            delete_segment(&mut socket, "audio").await;
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let engine = Arc::new(
            SessionOrchestrator::new_realtime(settings, None, Some(path.clone()), None).unwrap(),
        );
        let delivery_owner = Arc::clone(&engine);
        timeout(
            Duration::from_secs(10),
            tokio::task::spawn_blocking(move || {
                let id = SessionId(1);
                engine.start_session(id).unwrap();
                engine.on_chunk_ready(
                    id,
                    WavChunk::from_encoded_bytes(
                        include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
                    ),
                );
                let confirmation = engine
                    .finish_for_delivery(id)
                    .unwrap()
                    .confirmation
                    .unwrap();
                if confirmed {
                    confirmation.confirm("Use Codex.".into()).unwrap();
                    engine.shutdown();
                    let file: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    assert_eq!(file["observations"][0]["text"], "Use Codex.");
                    assert_eq!(file["corrections"][0]["rule"]["to"], "Codex");
                } else {
                    engine.shutdown();
                    assert!(confirmation.confirm("Use Codex.".into()).is_err());
                    assert!(!path.exists(), "shutdown must not learn unconfirmed text");
                }
            }),
        )
        .await
        .unwrap()
        .unwrap();
        drop(delivery_owner);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn shutdown_does_not_wait_for_an_in_flight_recording_response() {
    // Exiting while the service is silent must cancel network I/O, rather than
    // waiting for a response timeout while joining the memory worker.
    let (listener, settings) = listen().await;
    let (waiting, blocked) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        commit_segment(&mut socket, "audio").await;
        expect_event(&mut socket, "response.create").await;
        waiting.send(()).unwrap();
        resume.await.unwrap();
    });
    let engine = SessionOrchestrator::new_realtime(settings, None, None, None).unwrap();
    let id = SessionId(1);
    engine.start_session(id).unwrap();
    let frame = WavChunk::from_encoded_bytes(
        include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
    );
    for _ in 0..11 {
        engine.on_chunk_ready(id, frame.clone());
    }
    timeout(Duration::from_secs(10), blocked)
        .await
        .unwrap()
        .unwrap();
    timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || engine.shutdown()),
    )
    .await
    .unwrap()
    .unwrap();
    release.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn disconnect_after_completed_response_preserves_only_validated_text() {
    for valid in [true, false] {
        let (listener, settings) = listen().await;
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            commit_segment(&mut socket, "audio").await;
            let event = expect_event(&mut socket, "response.create").await;
            let text = if valid {
                json!({"text":"已完成。", "context":"工作", "memory_candidates":[]}).to_string()
            } else {
                "not JSON".into()
            };
            complete_response(&mut socket, &event, "r", &text).await;
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
            commit_segment(&mut socket, "audio-1").await;
            let event = expect_event(&mut socket, "response.create").await;
            let text = json!({"text":"保留这一段。", "context":"工作", "memory_candidates":[]})
                .to_string();
            complete_response(&mut socket, &event, "r1", &text).await;
            delete_segment(&mut socket, "audio-1").await;
            declare_prefix(&mut socket, "prefix-audio-2").await;
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
            delete_segment(&mut socket, "audio-2").await;
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
        let (engine, error) = timeout(
            Duration::from_secs(15),
            tokio::task::spawn_blocking(move || {
                let error = engine.finish_session(id).unwrap_err();
                (engine, error)
            }),
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
        drop(engine); // Ordinary abort cleanup runs while the application is still alive.
        assert!(
            !path.exists(),
            "failed or cancelled recording must not learn"
        );
    }
}

#[tokio::test]
async fn provisional_text_precedes_completion_but_only_validated_final_text_is_deliverable() {
    // A server can correct its streamed draft, truncate after streaming a plausible
    // sentence, or still be generating when Esc cancels a detached finalizer.
    for outcome in ["completed", "incomplete", "mismatched_final", "cancelled"] {
        let (listener, settings) = listen().await;
        let (release, resume) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            commit_segment(&mut socket, "audio").await;
            let request = expect_event(&mut socket, "response.create").await;
            send_json(&mut socket, json!({"type":"response.created","response":{"id":"r","metadata":request["response"]["metadata"]}})).await;
            let delta = json!({"type":"response.output_text.delta", "event_id":"draft", "response_id":"r", "output_index":0, "content_index":0,
                "delta":r#"{"memory_candidates":["must stay hidden"],"text":"我们明天下午去北京。"#});
            send_json(&mut socket, delta.clone()).await;
            send_json(&mut socket, delta).await; // Replayed events cannot duplicate the body.
            resume.await.unwrap(); // No completion is sent until the consumer sees the draft.
            if outcome == "cancelled" {
                let event = expect_event(&mut socket, "response.cancel").await;
                assert_eq!(event["response_id"], "r");
                send_json(
                    &mut socket,
                    json!({"type":"response.done","response":{"id":"r","status":"cancelled"}}),
                )
                .await;
                delete_segment(&mut socket, "audio").await;
                expect_event(&mut socket, "input_audio_buffer.clear").await;
                send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
            } else {
                let text = json!({"text":"我们明天上午去上海。"}).to_string();
                send_json(&mut socket, json!({"type":"response.output_text.done","response_id":"r","output_index":0,"content_index":0,"text":text})).await;
                let status = if outcome == "mismatched_final" {
                    "completed"
                } else {
                    outcome
                };
                let completed_text = if outcome == "mismatched_final" {
                    r#"{"text":"different final"}"#
                } else {
                    &text
                };
                send_json(&mut socket, json!({"type":"response.done","response":{"id":"r","status":status,"output":[{"content":[{"type":"output_text","text":completed_text}]}]}})).await;
                if outcome == "completed" {
                    delete_segment(&mut socket, "audio").await;
                }
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let engine = Arc::new(
            SessionOrchestrator::new_realtime(
                settings,
                None,
                Some(path.clone()),
                Some("zh".into()),
            )
            .unwrap(),
        );
        let (notify, mut updates) = tokio::sync::mpsc::channel(1);
        let progress = super::Progress::new(move || {
            let _ = notify.try_send(());
        });
        let id = SessionId(1);
        engine
            .start_session_with_progress(id, Some(progress.clone()))
            .unwrap();
        engine.on_chunk_ready(
            id,
            WavChunk::from_encoded_bytes(
                include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
            ),
        );
        let finalizer = engine.clone();
        let finished = tokio::task::spawn_blocking(move || finalizer.finish_for_delivery(id));
        timeout(Duration::from_secs(5), updates.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(progress.latest(), "我们明天下午去北京。");
        assert!(
            !finished.is_finished(),
            "a delta must not finish recognition"
        );
        assert!(!path.exists(), "provisional text must not be learned");
        if outcome == "cancelled" {
            progress.cancel();
        }
        release.send(()).unwrap();
        let result = timeout(Duration::from_secs(5), finished)
            .await
            .unwrap()
            .unwrap();
        match (outcome, result) {
            ("completed", Ok(transcript)) => {
                assert_eq!(transcript.text, "我们明天上午去上海。");
                assert_eq!(progress.latest(), transcript.text);
                drop(transcript); // No confirmation: no history/memory side effect.
            }
            (_, Err(SessionError::PartialFailure { partial_text, .. })) => {
                assert!(partial_text.is_empty())
            }
            other => panic!("unexpected stream outcome: {other:?}"),
        }
        engine.shutdown();
        assert!(!path.exists());
        server.await.unwrap();
    }
}
