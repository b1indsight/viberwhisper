use super::*;
use crate::dictation::{RealtimeMode, system_clock, test_support::*};
use crate::{core::orchestrator::SessionOrchestrator, session::SessionId};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::time::{Duration, timeout};

const ASR: &str =
    "You are a speech recognition model.\n请将这段中文语音转换为纯文本，去掉标点符号。";

fn short_speech() -> Vec<i16> {
    let wav = WavChunk::from_encoded_bytes(
        include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
    );
    let (rate, pcm) = crate::audio::decode_mono(&wav).unwrap();
    let pcm = PcmResampler::new(rate, 24_000)
        .unwrap()
        .push(&pcm, true)
        .unwrap();
    // Choose a real 500 ms speech excerpt using the same production gate; DC
    // tones are not speech and would exercise silence cleanup instead of ASR.
    pcm.chunks_exact(12_000)
        .find(|samples| {
            crate::audio::contains_speech_for_dictation(
                &crate::audio::chunk::encode_i16_wav(samples, 24_000).unwrap(),
            )
            .unwrap()
        })
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn transcription_short_segments_have_only_own_audio_and_no_memory_owner() {
    // A pause can produce a short segment anywhere in a recording. Vocabulary,
    // previous output and the integrated prompt must never become ASR inputs.
    let (listener, mut settings) = listen().await;
    settings.prompt = Some("integrated-only sentinel".into());
    settings.memory.long_term = serde_json::from_value(json!([{
        "context":"project", "kind":"proper_noun", "text":"private vocabulary sentinel"
    }]))
    .unwrap();
    let first = short_speech();
    let tail = first
        .chunks_exact(600)
        .max_by_key(|samples| {
            samples
                .iter()
                .map(|sample| i64::from(*sample).pow(2))
                .sum::<i64>()
        })
        .unwrap()
        .to_vec();
    let mut pcm = first.clone();
    pcm.extend(vec![0; 48_000]);
    pcm.extend(tail.clone());
    let expected_audio = pcm.clone();
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        let update = expect_event(&mut socket, "session.update").await;
        assert_eq!(update["session"]["instructions"], ASR);
        send_json(&mut socket, json!({"type":"session.updated"})).await;
        let mut uploaded = Vec::new();
        for (index, text) in ["嗯", "继续说下去"].into_iter().enumerate() {
            loop {
                let message = socket.next().await.unwrap().unwrap();
                let event: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                if event["type"] == "input_audio_buffer.commit" {
                    break;
                }
                assert_eq!(
                    event["type"], "input_audio_buffer.append",
                    "no text item may precede audio"
                );
                uploaded.extend(STANDARD.decode(event["audio"].as_str().unwrap()).unwrap());
            }
            let audio = format!("audio-{index}");
            send_json(
                &mut socket,
                json!({"type":"input_audio_buffer.committed", "item_id":audio}),
            )
            .await;
            let request = expect_event(&mut socket, "response.create").await;
            assert_eq!(request["response"]["instructions"], ASR);
            assert_eq!(request["response"]["conversation"], "none");
            assert_eq!(
                request["response"]["input"],
                json!([{"type":"item_reference", "id":audio}])
            );
            let id = format!("r-{index}");
            send_json(&mut socket, json!({"type":"response.created", "response":{"id":id,"metadata":request["response"]["metadata"]}})).await;
            for (part, delta) in text.chars().enumerate() {
                let event = json!({"type":"response.output_text.delta", "event_id":format!("d-{index}-{part}"), "response_id":id,"output_index":0,"content_index":0,"delta":delta.to_string()});
                send_json(&mut socket, event.clone()).await;
                send_json(&mut socket, event).await;
            }
            send_json(&mut socket, json!({"type":"response.output_text.done", "response_id":id,"output_index":0,"content_index":0,"text":text})).await;
            send_json(&mut socket, json!({"type":"response.done", "response":{"id":id,"status":"completed","output":[{"content":[{"type":"output_text","text":text}]}]}})).await;
            delete_audio(&mut socket, &audio).await;
        }
        let bytes: Vec<_> = expected_audio
            .iter()
            .flat_map(|s: &i16| s.to_le_bytes())
            .collect();
        assert_eq!(
            uploaded, bytes,
            "short voiced tails must survive without trimming or padding"
        );
    });
    let directory = tempfile::tempdir().unwrap();
    let memory = directory.path().join("must-not-open");
    std::fs::write(&memory, "untouched private memory sentinel").unwrap();
    let client = tokio::task::spawn_blocking(move || {
        // Memory loading evaluates its clock before any file read; fail if even
        // ephemeral memory or a per-segment snapshot is constructed in pure ASR.
        let engine = SessionOrchestrator::new_realtime(
            settings,
            None,
            Some(memory.clone()),
            Some("zh".into()),
            RealtimeMode::Transcription,
            Arc::new(|| panic!("ASR must not use the memory clock")),
            None,
        )
        .unwrap();
        assert!(
            engine
                .snapshot()
                .unwrap_err()
                .to_string()
                .contains("does not own memory")
        );
        let id = SessionId(1);
        let progress = Progress::new(|| {});
        engine
            .start_session_with_progress(id, Some(progress.clone()))
            .unwrap();
        for packet in pcm.chunks(4800) {
            engine.on_chunk_ready(
                id,
                crate::audio::chunk::encode_i16_wav(packet, 24_000).unwrap(),
            );
        }
        let transcript = engine.finish_for_delivery(id).unwrap();
        assert_eq!(transcript.text, "嗯继续说下去");
        assert_eq!(progress.latest(), transcript.text);
        assert!(transcript.confirmation.is_none());
        let traces = engine.take_traces();
        assert_eq!(traces.len(), 2);
        for (index, trace) in traces.iter().enumerate() {
            assert_eq!(trace.index, index);
            assert_eq!(trace.input_layout, "audio-only-v1");
            assert!(trace.input.is_none());
            assert!(trace.prompt_input.is_none());
            assert!(trace.at.is_none());
            assert_eq!(trace.instructions.as_deref(), Some(ASR));
            assert_eq!(trace.output, json!(["嗯", "继续说下去"][index]));
        }
        assert_eq!(traces[0].audio_start_sample, Some(0));
        assert_eq!(traces[0].audio_end_sample, traces[1].audio_start_sample);
        assert_eq!(traces[1].audio_end_sample, Some(pcm.len()));
        assert!(engine.take_traces().is_empty());
        assert_eq!(
            std::fs::read_to_string(memory).unwrap(),
            "untouched private memory sentinel"
        );
    });
    timeout(Duration::from_secs(10), async {
        client.await.unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn transcription_candidate_is_connection_local_and_plain_text_is_never_reinterpreted() {
    // Explicit --no-prompt is distinct from the stored integrated override. A
    // JSON-looking utterance remains literal body text, with no metadata learning.
    for instructions in ["", "候选纯转写指令"] {
        let (listener, mut settings) = listen().await;
        settings.prompt = Some("saved integrated prompt".into());
        let saved = settings.clone();
        let text = "{\"text\":\"literal\",\"context\":\"spoken\"}";
        let server = tokio::spawn(async move {
            let mut socket = accept(&listener).await;
            assert_eq!(
                expect_event(&mut socket, "session.update").await["session"]["instructions"],
                instructions
            );
            send_json(&mut socket, json!({"type":"session.updated"})).await;
            for index in 0..2 {
                let audio = format!("audio-{index}");
                commit_audio(&mut socket, &audio).await;
                let request = expect_event(&mut socket, "response.create").await;
                assert_eq!(request["response"]["instructions"], instructions);
                assert_eq!(
                    request["response"]["input"],
                    json!([{"type":"item_reference", "id":audio}])
                );
                complete_response(&mut socket, &request, &format!("r-{index}"), text).await;
                delete_audio(&mut socket, &audio).await;
            }
        });
        let client = tokio::task::spawn_blocking(move || {
            let engine = SessionOrchestrator::new_realtime(
                settings.clone(),
                None,
                None,
                None,
                RealtimeMode::Transcription,
                system_clock(),
                Some(instructions.into()),
            )
            .unwrap();
            for index in 0..2 {
                let id = SessionId(index);
                engine.start_session(id).unwrap();
                engine.on_chunk_ready(
                    id,
                    crate::audio::chunk::encode_i16_wav(&short_speech(), 24_000).unwrap(),
                );
                let result = engine.finish_for_delivery(id).unwrap();
                assert_eq!(result.text, text);
                assert!(result.confirmation.is_none());
                let trace = engine.take_traces().pop().unwrap();
                assert_eq!(trace.output, json!(text));
                assert!(trace.input.is_none());
            }
            assert!(settings == saved);
        });
        timeout(Duration::from_secs(10), async {
            client.await.unwrap();
            server.await.unwrap();
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn integrated_live_clock_controls_segment_memory_and_confirmation_barrier() {
    use std::sync::atomic::{AtomicU64, Ordering};
    // A long-running recording crosses retention expiry. Later segments must
    // use the live injected time, while confirmation and snapshots share that clock.
    let (listener, mut settings) = listen().await;
    let seed: crate::dictation::MemorySeed = serde_json::from_value(json!({
        "long_term":[{"context":"project","kind":"proper_noun","text":"ViberWhisper"}],
        "observations":[{"recording":"prior", "segment":0,"at":99,"context":"project","text":"seed history","terms":[]}]
    })).unwrap();
    settings.memory.long_term = seed.long_term.clone();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("seeded.json");
    seed.write(&path, 100).unwrap();
    let time = Arc::new(AtomicU64::new(100));
    let server_time = Arc::clone(&time);
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        for (index, history) in [
            json!(["seed history"]),
            json!(["segment0", "seed history"]),
            // At 86_500 the seed and segment0 are at least a day old;
            // segment1 was learned at 110 and has ten seconds left.
            json!(["segment1"]),
        ]
        .into_iter()
        .enumerate()
        {
            let audio = format!("audio-{index}");
            commit_segment(&mut socket, &audio).await;
            let request = expect_event(&mut socket, "response.create").await;
            let dynamic: Value = serde_json::from_str(
                request["response"]["input"][2]["content"][0]["text"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(dynamic["contexts"][0]["history"], history);
            complete_response(&mut socket, &request, &format!("r-{index}"), &json!({"text":format!("segment{index}"),"context":"project","memory_candidates":[]}).to_string()).await;
            if index == 0 {
                server_time.store(110, Ordering::SeqCst);
            } else {
                server_time.store(86_500, Ordering::SeqCst);
            }
            delete_segment(&mut socket, &audio).await;
        }
    });
    let client = tokio::task::spawn_blocking(move || {
        let clock_time = Arc::clone(&time);
        let engine = SessionOrchestrator::new_realtime(
            settings,
            None,
            Some(path.clone()),
            None,
            RealtimeMode::Integrated,
            Arc::new(move || clock_time.load(Ordering::SeqCst)),
            None,
        )
        .unwrap();
        let (snapshot, snapshot_at) = engine.snapshot_with_time().unwrap();
        assert_eq!(snapshot.contexts[0].history, vec!["seed history"]);
        assert_eq!(snapshot_at, 100);
        let id = SessionId(1);
        engine.start_session(id).unwrap();
        let mut pcm = short_speech();
        pcm.extend(vec![0; 48_000]);
        pcm.extend(short_speech());
        pcm.extend(vec![0; 48_000]);
        pcm.extend(short_speech());
        engine.on_chunk_ready(
            id,
            crate::audio::chunk::encode_i16_wav(&pcm, 24_000).unwrap(),
        );
        let result = engine.finish_for_delivery(id).unwrap();
        assert_eq!(result.text, "segment0 segment1 segment2");
        let traces = engine.take_traces();
        assert_eq!(traces.len(), 3);
        assert_eq!(traces[0].at, Some(100));
        assert_eq!(traces[1].at, Some(110));
        assert_eq!(traces[2].at, Some(86_500));
        assert!(
            traces[0].input.as_ref().unwrap().contexts[0]
                .history
                .contains(&"seed history".into())
        );
        assert!(
            traces[1].input.as_ref().unwrap().contexts[0]
                .history
                .contains(&"segment0".into())
        );
        assert_eq!(
            traces[2].input.as_ref().unwrap().contexts[0].history,
            vec!["segment1"]
        );
        // Validate the live observations, including mid-recording expiry, through
        // historical replay without committing the recording's transient memory.
        crate::dictation::Replay::new(&seed, 100)
            .replay("clocked", 100, &traces, false)
            .unwrap();
        time.store(86_510, Ordering::SeqCst);
        let confirmed_at = result
            .confirmation
            .unwrap()
            .confirm_with_ack(result.text)
            .unwrap();
        assert_eq!(confirmed_at, 86_510);
        // Reporting can happen in a later second than the actual confirmation.
        time.store(86_520, Ordering::SeqCst);
        let (snapshot, snapshot_at) = engine.snapshot_with_time().unwrap();
        assert_eq!(snapshot_at, 86_520);
        assert!(
            !snapshot.history_is_empty(),
            "snapshot must wait for accepted feedback"
        );
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let observations = saved["observations"].as_array().unwrap();
        assert_eq!(observations.len(), 3);
        assert!(observations.iter().all(|o| o["at"] == confirmed_at));
        time.store(172_910, Ordering::SeqCst);
        assert!(engine.snapshot().unwrap().history_is_empty());
    });
    timeout(Duration::from_secs(10), async {
        client.await.unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn transcription_rejects_inconsistent_or_invalid_streams_through_public_worker() {
    // Provisional ASR text cannot be promoted if status, correlation, indexing or
    // either authoritative completion disagrees. Plain-text limits apply in flight.
    for fault in [
        "delta",
        "completion",
        "status",
        "correlation",
        "index",
        "duplicate_done",
        "nul",
        "limit",
        "done_nul",
        "done_limit",
    ] {
        let (listener, settings) = listen().await;
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            commit_audio(&mut socket, "audio").await;
            let request = expect_event(&mut socket, "response.create").await;
            let correlation = if fault == "correlation" {
                json!({"dictation_id":"wrong"})
            } else {
                request["response"]["metadata"].clone()
            };
            send_json(
                &mut socket,
                json!({"type":"response.created", "response":{"id":"r","metadata":correlation}}),
            )
            .await;
            if fault == "correlation" {
                return;
            }
            let delta = match fault {
                "delta" => "别的".into(),
                "nul" => "正文\0".into(),
                "limit" => "a".repeat(16_385),
                _ => "正文".to_owned(),
            };
            if !fault.starts_with("done_") {
                send_json(&mut socket, json!({"type":"response.output_text.delta", "response_id":"r","output_index":if fault == "index" {1} else {0},"content_index":0,"delta":delta})).await;
            }
            if ["index", "nul", "limit"].contains(&fault) {
                return;
            }
            let done_text = match fault {
                "done_nul" => "正文\0".into(),
                "done_limit" => "a".repeat(16_385),
                _ => "正文".to_owned(),
            };
            let done = json!({"type":"response.output_text.done", "response_id":"r","output_index":0,"content_index":0,"text":done_text});
            send_json(&mut socket, done.clone()).await;
            if fault.starts_with("done_") {
                return;
            }
            if fault == "duplicate_done" {
                send_json(&mut socket, done).await;
                return;
            }
            send_json(&mut socket, json!({"type":"response.done", "response":{"id":"r","status":if fault == "status" {"incomplete"} else {"completed"},"output":[{"content":[{"type":"output_text","text":if fault == "completion" {"不同正文"} else {"正文"}}]}]}})).await;
        });
        let client = tokio::task::spawn_blocking(move || {
            let engine = SessionOrchestrator::new_realtime(
                settings,
                None,
                None,
                None,
                RealtimeMode::Transcription,
                system_clock(),
                None,
            )
            .unwrap();
            let id = SessionId(1);
            engine.start_session(id).unwrap();
            engine.on_chunk_ready(
                id,
                crate::audio::chunk::encode_i16_wav(&short_speech(), 24_000).unwrap(),
            );
            let SessionError::PartialFailure {
                partial_text,
                errors,
            } = engine.finish_for_delivery(id).unwrap_err()
            else {
                panic!("expected protocol failure: {fault}");
            };
            assert!(partial_text.is_empty(), "{fault}");
            let TranscribeError::Network(message) = &errors[0].1 else {
                panic!("expected protocol error");
            };
            let expected = match fault {
                "delta" => "delta/text mismatch",
                "completion" => "completion/text mismatch",
                "status" => "truncated",
                "correlation" => "correlation",
                "index" => "content part",
                "duplicate_done" => "duplicate",
                "nul" | "done_nul" => "invalid text",
                "limit" | "done_limit" => "too long",
                _ => unreachable!(),
            };
            assert!(message.contains(expected), "{fault}: {message}");
        });
        timeout(Duration::from_secs(10), async {
            client.await.unwrap();
            server.await.unwrap();
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn transcription_cancellation_drains_response_and_deletes_only_audio() {
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        commit_audio(&mut socket, "audio").await;
        let request = expect_event(&mut socket, "response.create").await;
        send_json(&mut socket, json!({"type":"response.created", "response":{"id":"r","metadata":request["response"]["metadata"]}})).await;
        send_json(&mut socket, json!({"type":"response.output_text.delta", "response_id":"r","output_index":0,"content_index":0,"delta":"临时正文"})).await;
        assert_eq!(
            expect_event(&mut socket, "response.cancel").await["response_id"],
            "r"
        );
        send_json(
            &mut socket,
            json!({"type":"response.done", "response":{"id":"r", "status":"cancelled"}}),
        )
        .await;
        delete_audio(&mut socket, "audio").await;
        expect_event(&mut socket, "input_audio_buffer.clear").await;
        send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
    });
    let engine = Arc::new(
        SessionOrchestrator::new_realtime(
            settings,
            None,
            None,
            None,
            RealtimeMode::Transcription,
            system_clock(),
            None,
        )
        .unwrap(),
    );
    let (notify, mut updates) = tokio::sync::mpsc::channel(1);
    let progress = Progress::new(move || {
        let _ = notify.try_send(());
    });
    let id = SessionId(1);
    engine
        .start_session_with_progress(id, Some(progress.clone()))
        .unwrap();
    engine.on_chunk_ready(
        id,
        crate::audio::chunk::encode_i16_wav(&short_speech(), 24_000).unwrap(),
    );
    let finalizer = Arc::clone(&engine);
    let finished = tokio::task::spawn_blocking(move || finalizer.finish_for_delivery(id));
    timeout(Duration::from_secs(5), updates.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.latest(), "临时正文");
    assert!(!finished.is_finished());
    progress.cancel();
    let result = timeout(Duration::from_secs(5), finished)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(SessionError::PartialFailure { partial_text, .. }) if partial_text.is_empty())
    );
    assert!(engine.take_traces().is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn transcription_cleanup_failure_preserves_verified_text_and_actual_trace() {
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        commit_audio(&mut socket, "audio").await;
        let request = expect_event(&mut socket, "response.create").await;
        complete_response(&mut socket, &request, "r", "保留正文").await;
        socket.close(None).await.unwrap();
    });
    let client = tokio::task::spawn_blocking(move || {
        let engine = SessionOrchestrator::new_realtime(
            settings,
            None,
            None,
            None,
            RealtimeMode::Transcription,
            system_clock(),
            None,
        )
        .unwrap();
        let id = SessionId(1);
        engine.start_session(id).unwrap();
        engine.on_chunk_ready(
            id,
            crate::audio::chunk::encode_i16_wav(&short_speech(), 24_000).unwrap(),
        );
        assert!(
            matches!(engine.finish_for_delivery(id), Err(SessionError::PartialFailure { partial_text, .. }) if partial_text == "保留正文")
        );
        let traces = engine.take_traces();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].output, json!("保留正文"));
        assert!(traces[0].input.is_none());
    });
    timeout(Duration::from_secs(10), async {
        client.await.unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}
