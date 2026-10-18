use super::*;
use crate::dictation::test_support::{
    accept, accept_session, commit_audio, commit_segment, complete_response, declare_prefix,
    delete_audio, delete_segment, expect_event, listen, send_json,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn recording_declares_prefix_before_audio_and_orders_final_input() {
    use crate::{core::orchestrator::SessionOrchestrator, session::SessionId};
    // Previously the first client item was audio, and all context arrived before
    // the audio reference only at stop. Exercise the actual desktop worker path.
    let (listener, mut settings) = listen().await;
    settings.memory = serde_json::from_value(json!({"long_term":[{
        "context":"project", "kind":"proper_noun", "text":"ViberWhisper"
    }]}))
    .unwrap();
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        let declared = expect_event(&mut socket, "conversation.item.create").await;
        assert_eq!(declared["item"]["role"], "user");
        let stable: Value =
            serde_json::from_str(declared["item"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            stable["contexts"][0]["long_term"][0]["text"],
            "ViberWhisper"
        );
        let mut item = declared["item"].clone();
        item["id"] = json!("prefix");
        item["status"] = json!("completed");
        for kind in ["conversation.item.added", "conversation.item.done"] {
            send_json(&mut socket, json!({"type":kind,"item":item})).await;
        }
        expect_event(&mut socket, "input_audio_buffer.append").await;
        commit_audio(&mut socket, "audio").await;
        let request = expect_event(&mut socket, "response.create").await;
        assert_eq!(request["response"]["conversation"], "none");
        assert_eq!(request["response"]["input"].as_array().unwrap().len(), 3);
        assert_eq!(
            request["response"]["input"][0],
            json!({"type":"item_reference","id":"prefix"})
        );
        assert_eq!(
            request["response"]["input"][1],
            json!({"type":"item_reference","id":"audio"})
        );
        assert_eq!(
            request["response"]["input"][2]["content"][0]["text"],
            "{\"contexts\":[]}"
        );
        complete_response(
            &mut socket,
            &request,
            "response",
            r#"{"memory_candidates":[],"context":{"kind":"uncertain"},"text":"测试"}"#,
        )
        .await;
        delete_audio(&mut socket, "audio").await;
        delete_audio(&mut socket, "prefix").await;
    });
    let client = tokio::task::spawn_blocking(move || {
        let engine = SessionOrchestrator::new_realtime(settings, None, None, None).unwrap();
        let id = SessionId(0);
        engine.start_session(id).unwrap();
        engine.on_chunk_ready(
            id,
            crate::audio::WavChunk::from_encoded_bytes(
                include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
            ),
        );
        engine.finish_for_delivery(id).unwrap().text
    });
    let (text, ()) = timeout(Duration::from_secs(10), async {
        (client.await.unwrap(), server.await.unwrap())
    })
    .await
    .unwrap();
    assert_eq!(text, "测试");
}

#[tokio::test]
async fn upload_preserves_protocol_events_in_a_bounded_queue() {
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        let event = expect_event(&mut socket, "session.update").await;
        assert_eq!(event["session"]["instructions"], "");
        send_json(&mut socket, json!({"type":"session.updated"})).await;
        for sequence in 0..33 {
            send_json(
                &mut socket,
                json!({"type":"rate_limits.updated","sequence":sequence}),
            )
            .await;
        }
    });
    let mut connection = Connection::connect(
        &DictationConfig {
            prompt: Some(String::new()),
            ..settings
        },
        None,
    )
    .await
    .unwrap();
    for _ in 0..32 {
        connection.receive_while_recording().await.unwrap();
    }
    assert!(
        connection
            .receive_while_recording()
            .await
            .unwrap_err()
            .to_string()
            .contains("capacity")
    );
    for sequence in 0..32 {
        assert_eq!(connection.receive().await.unwrap()["sequence"], sequence);
    }
    server.await.unwrap();
}

#[tokio::test]
async fn recording_answers_heartbeat_and_reports_server_error_before_stop() {
    use crate::{core::orchestrator::SessionOrchestrator, session::SessionId};
    let (listener, settings) = listen().await;
    let (heartbeat, answered) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        declare_prefix(&mut socket, "prefix").await;
        expect_event(&mut socket, "input_audio_buffer.append").await;
        socket
            .send(Message::Ping(b"during-recording".to_vec().into()))
            .await
            .unwrap();
        // Recording stays open awaiting Pong; stop/commit cannot service the heartbeat.
        loop {
            match timeout(Duration::from_secs(2), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                Message::Text(text) => {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(event["type"], "input_audio_buffer.append");
                }
                Message::Pong(payload) => {
                    assert_eq!(&payload[..], b"during-recording");
                    break;
                }
                other => panic!("unexpected frame: {other:?}"),
            }
        }
        send_json(
            &mut socket,
            json!({"type":"error","error":{"code":"test_upload_failure"}}),
        )
        .await;
        heartbeat.send(()).unwrap();
        // Keep the connection open until the client handles the error.
        while socket.next().await.is_some() {}
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let engine =
        SessionOrchestrator::new_realtime(settings, None, Some(path.clone()), None).unwrap();
    let id = SessionId(42);
    engine.start_session(id).unwrap();
    engine.on_chunk_ready(
        id,
        crate::audio::chunk::encode_i16_wav(&[1000; 16_000], 16_000).unwrap(),
    );
    timeout(Duration::from_secs(3), answered)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            if let Some(error) = engine.recording_error(id) {
                assert!(error.contains("test_upload_failure"));
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("server error must be observed before stop");
    assert!(engine.finish_session(id).is_err());
    assert!(!path.exists());
    server.await.unwrap();
}

#[tokio::test]
async fn truncated_response_is_rejected_even_when_text_done_looks_valid() {
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        commit_segment(&mut socket, "audio").await;
        let event = expect_event(&mut socket, "response.create").await;
        // Text completion alone must not deliver a response cut off by the token limit.
        for reply in [
            json!({"type":"response.created","response":{"id":"r","metadata":event["response"]["metadata"]}}),
            json!({"type":"response.output_text.done","response_id":"r","output_index":0,"content_index":0,"text":"{}"}),
            json!({"type":"response.done","response":{"id":"r","status":"incomplete","status_details":{"reason":"max_output_tokens"}}}),
        ] {
            send_json(&mut socket, reply).await;
        }
    });
    let mut connection = Connection::connect(&settings, None).await.unwrap();
    let memory = crate::dictation::memory::Memory::frozen(Default::default());
    let input = memory.prompt_input(&memory.stable_prefix(), 0);
    connection.begin(&input.stable, "truncated").await.unwrap();
    connection.append(&[1; 2400]).await.unwrap();
    let error = connection.finish(&input, "truncated").await.unwrap_err();
    assert!(error.to_string().contains("truncated"));
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_between_create_and_ack_still_cancels_the_correct_response() {
    let (listener, settings) = listen().await;
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept_session(&listener).await;
        commit_segment(&mut socket, "audio").await;
        let event = expect_event(&mut socket, "response.create").await;
        // Stop finish() while its response ID is still awaiting acknowledgement.
        ready.send(()).unwrap();
        resume.await.unwrap();
        send_json(&mut socket, json!({"type":"response.created","response":{"id":"pending-r","metadata":event["response"]["metadata"]}})).await;
        let event = expect_event(&mut socket, "response.cancel").await;
        assert_eq!(event["response_id"], "pending-r");
        send_json(
            &mut socket,
            json!({"type":"response.done","response":{"id":"pending-r","status":"cancelled"}}),
        )
        .await;
        delete_segment(&mut socket, "audio").await;
        expect_event(&mut socket, "input_audio_buffer.clear").await;
        send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
    });
    let mut connection = Connection::connect(&settings, None).await.unwrap();
    let memory = crate::dictation::memory::Memory::frozen(Default::default());
    let input = memory.prompt_input(&memory.stable_prefix(), 0);
    connection.begin(&input.stable, "cancelled").await.unwrap();
    tokio::select! {
        result = connection.finish(&input, "cancelled") => panic!("unexpected completion {result:?}"),
        _ = waiting => {}
    }
    release.send(()).unwrap();
    connection.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "requires an explicitly configured live Realtime service"]
async fn live_realtime_structured_multiturn() {
    let settings = crate::dictation::config::DictationConfig {
        url: std::env::var("VIBERWHISPER_REALTIME_TEST_URL")
            .expect("set VIBERWHISPER_REALTIME_TEST_URL"),
        model: std::env::var("VIBERWHISPER_REALTIME_TEST_MODEL")
            .unwrap_or_else(|_| DictationConfig::default().model),
        ..Default::default()
    };
    let key = std::env::var("REALTIME_API_KEY").ok();
    let mut connection = Connection::connect(&settings, key.as_deref())
        .await
        .unwrap();
    let mut reader = hound::WavReader::new(std::io::Cursor::new(include_bytes!(
        "../../../tests/fixtures/audio/speech.wav"
    )))
    .unwrap();
    let rate = reader.spec().sample_rate;
    let samples: Vec<i16> = reader.samples().collect::<Result<_, _>>().unwrap();
    let pcm = crate::audio::PcmResampler::new(rate)
        .unwrap()
        .push(&samples, true)
        .unwrap();
    let memory = crate::dictation::memory::Memory::frozen(Default::default());
    let input = memory.prompt_input(&memory.stable_prefix(), 0);
    for round in 0..2 {
        let correlation = format!("live-{round}");
        connection.begin(&input.stable, &correlation).await.unwrap();
        connection.append(&pcm).await.unwrap();
        let result = connection.finish(&input, &correlation).await.unwrap();
        assert!(!result.text.is_empty());
        connection.delete_segment().await.unwrap();
        println!(
            "round {round}: valid structured result, {} characters",
            result.text.chars().count()
        );
    }
}

#[tokio::test]
async fn recordings_reuse_connection_over_sixty_seconds_and_preserve_repetitions() {
    use crate::{core::orchestrator::SessionOrchestrator, session::SessionId};
    const PROMPT: &str = "候选完整基础提示词";
    let (listener, settings) = listen().await;
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener).await;
        let mut bytes = 0;
        let mut round = 0;
        while let Some(Ok(message)) = socket.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let event: serde_json::Value = serde_json::from_str(&text).unwrap();
            let mut replies = Vec::new();
            match event["type"].as_str().unwrap() {
                "session.update" => {
                    assert_eq!(event["session"]["instructions"], PROMPT);
                    assert!(event["session"]["audio"]["input"]["turn_detection"].is_null());
                    assert!(event["session"]["audio"]["input"].get("transcription").is_none());
                    replies.push(json!({"type":"session.updated"}));
                }
                "conversation.item.create" => {
                    let mut item = event["item"].clone();
                    item["id"] = json!(format!("prefix-{round}"));
                    item["status"] = json!("completed");
                    replies.push(json!({"type":"conversation.item.added", "item":item}));
                    replies.push(json!({"type":"conversation.item.done", "item":item}));
                }
                "input_audio_buffer.append" => {
                    use base64::Engine;
                    bytes += base64::engine::general_purpose::STANDARD.decode(event["audio"].as_str().unwrap()).unwrap().len();
                }
                "input_audio_buffer.commit" => replies.push(json!({"type":"input_audio_buffer.committed", "item_id":format!("audio-{round}")})),
                "response.create" => {
                    assert_eq!(event["response"]["instructions"], PROMPT);
                    assert_eq!(event["response"]["conversation"], "none");
                    assert_eq!(event["response"]["input"][1]["id"], format!("audio-{round}"));
                    assert_eq!(event["response"]["input"][0]["id"], format!("prefix-{round}"));
                    assert_eq!(event["response"]["input"][2]["content"][0]["type"], "input_text");
                    let id = format!("response-{round}");
                    let text = json!({"text":"重复。", "context":{"kind":"new","label":"测试语境"}, "memory_candidates":[{"kind":"phrase","text":"重复","quote":"重复。"}]}).to_string();
                    replies.push(json!({"type":"response.created", "response":{"id":id, "metadata":event["response"]["metadata"]}}));
                    let delta = json!({"type":"response.output_text.delta", "event_id":format!("delta-{round}"), "response_id":id, "item_id":"out", "output_index":0,"content_index":0,"delta":text});
                    replies.push(delta.clone());
                    replies.push(delta); // Replayed delivery must not duplicate the JSON/text.
                    replies.push(json!({"type":"response.output_text.done", "response_id":id, "item_id":"out", "output_index":0,"content_index":0,"text":text}));
                    replies.push(json!({"type":"response.done", "response":{"id":id,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}));
                }
                "conversation.item.delete" => {
                    replies.push(json!({"type":"conversation.item.deleted","item_id":event["item_id"]}));
                    if event["item_id"] == format!("prefix-{round}") {
                        round += 1;
                    }
                }
                other => panic!("unexpected event: {other}"),
            }
            for reply in replies {
                send_json(&mut socket, reply).await;
            }
            if round == 4 {
                assert_eq!(bytes, 66 * 48_000);
                break;
            }
        }
    });
    let settings = crate::dictation::config::DictationConfig {
        prompt_components: Some(
            serde_json::from_value(json!({
                "task": PROMPT, "language": "", "numbers": "", "cleanup": "",
                "output_format": "", "context": "", "memory": ""
            }))
            .unwrap(),
        ),
        ..settings
    };
    let engine =
        std::sync::Arc::new(SessionOrchestrator::new_realtime(settings, None, None, None).unwrap());
    let mut reader = hound::WavReader::new(std::io::Cursor::new(include_bytes!(
        "../../../tests/fixtures/audio/speech.wav"
    )))
    .unwrap();
    let samples: Vec<i16> = reader.samples().collect::<Result<_, _>>().unwrap();
    for turn in 1..=2 {
        let engine = engine.clone();
        let samples = samples.clone();
        let text = tokio::task::spawn_blocking(move || {
            let id = SessionId(turn);
            engine.start_session(id).unwrap();
            // Two 33-second recordings share a socket and each produces two responses.
            for _ in 0..11 {
                for frame in samples.chunks(3200) {
                    engine.on_chunk_ready(
                        id,
                        crate::audio::chunk::encode_i16_wav(frame, 16_000).unwrap(),
                    );
                }
            }
            let transcript = engine.finish_for_delivery(id).unwrap();
            transcript
                .confirmation
                .unwrap()
                .confirm(transcript.text.clone())
                .unwrap();
            transcript.text
        })
        .await
        .unwrap();
        assert_eq!(text, "重复。 重复。");
    }
    server.await.unwrap();
}
