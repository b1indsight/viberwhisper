use super::*;
use crate::dictation::{
    memory::Memory,
    test_support::{
        accept_session, commit_audio, complete_response, declare_prefix, delete_audio,
        expect_event, listen, send_json,
    },
};

const RESULT: &str = r#"{"memory_candidates":[],"context":"未知语境","text":"测试"}"#;

#[tokio::test]
async fn cancellation_during_cleanup_never_repeats_an_item_delete() {
    // The worker can cancel after either delete was sent, including after audio's
    // deletion was acknowledged. Resume the pending ack without using deleted IDs again.
    for at_prefix in [false, true] {
        let (listener, settings) = listen().await;
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            declare_prefix(&mut socket, "prefix").await;
            commit_audio(&mut socket, "audio").await;
            let request = expect_event(&mut socket, "response.create").await;
            complete_response(&mut socket, &request, "response", RESULT).await;
            if at_prefix {
                delete_audio(&mut socket, "audio").await;
            }
            let pending = if at_prefix { "prefix" } else { "audio" };
            let event = expect_event(&mut socket, "conversation.item.delete").await;
            assert_eq!(event["item_id"], pending);
            ready.send(()).unwrap();
            resume.await.unwrap();
            send_json(
                &mut socket,
                json!({"type":"conversation.item.deleted","item_id":pending}),
            )
            .await;
            if !at_prefix {
                delete_audio(&mut socket, "prefix").await;
            }
            expect_event(&mut socket, "input_audio_buffer.clear").await;
            send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
        });
        let memory = Memory::frozen(Default::default());
        let input = memory.prompt_input(&memory.stable_prefix(), 0);
        let mut connection = Connection::connect(&settings, None).await.unwrap();
        connection.begin(&input.stable, "cleanup").await.unwrap();
        connection.append(&[1; 2400]).await.unwrap();
        connection.finish(&input, "cleanup").await.unwrap();
        tokio::select! {
            result = connection.delete_segment() => panic!("unexpected {result:?}"),
            _ = waiting => {}
        }
        release.send(()).unwrap();
        connection.cancel().await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn prefix_acknowledgement_rejects_mismatched_items_without_uploading() {
    // A done event for another item, an incomplete item or an altered payload
    // cannot authorize the microphone upload, even if added looked correct.
    for field in ["id", "status", "content"] {
        let (listener, settings) = listen().await;
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            let declared = expect_event(&mut socket, "conversation.item.create").await;
            let mut item = declared["item"].clone();
            item["id"] = json!("expected");
            item["status"] = json!("completed");
            send_json(
                &mut socket,
                json!({"type":"conversation.item.added","item":item}),
            )
            .await;
            match field {
                "id" => item["id"] = json!("another-item"),
                "status" => item["status"] = json!("in_progress"),
                _ => item["content"][0]["text"] = json!("altered"),
            }
            send_json(
                &mut socket,
                json!({"type":"conversation.item.done","item":item}),
            )
            .await;
            assert!(
                !matches!(socket.next().await, Some(Ok(Message::Text(_)))),
                "rejected prefix must not be followed by audio"
            );
        });
        let memory = Memory::frozen(Default::default());
        let mut connection = Connection::connect(&settings, None).await.unwrap();
        assert!(connection.append(&[1; 2400]).await.is_err());
        assert!(
            connection
                .begin(&memory.stable_prefix(), "rejected")
                .await
                .is_err()
        );
        assert!(connection.append(&[1; 2400]).await.is_err());
        drop(connection);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_resolves_pending_prefix_or_commit_before_clearing_items() {
    // Cancellation may drop begin/finish before the server allocates an item ID.
    // Late acknowledgements must clean up that segment, never become the next one's IDs.
    for at_commit in [false, true] {
        let (listener, settings) = listen().await;
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut socket = accept_session(&listener).await;
            let pending = if at_commit {
                declare_prefix(&mut socket, "pending-prefix").await;
                expect_event(&mut socket, "input_audio_buffer.commit").await;
                None
            } else {
                let declared = expect_event(&mut socket, "conversation.item.create").await;
                let mut item = declared["item"].clone();
                item["id"] = json!("pending-prefix");
                item["status"] = json!("completed");
                send_json(
                    &mut socket,
                    json!({"type":"conversation.item.added","item":item}),
                )
                .await;
                Some(item)
            };
            ready.send(()).unwrap();
            resume.await.unwrap();
            if let Some(item) = pending {
                send_json(
                    &mut socket,
                    json!({"type":"conversation.item.done","item":item}),
                )
                .await;
            } else {
                send_json(
                    &mut socket,
                    json!({"type":"input_audio_buffer.committed","item_id":"pending-audio"}),
                )
                .await;
                delete_audio(&mut socket, "pending-audio").await;
            }
            delete_audio(&mut socket, "pending-prefix").await;
            expect_event(&mut socket, "input_audio_buffer.clear").await;
            send_json(&mut socket, json!({"type":"input_audio_buffer.cleared"})).await;
            declare_prefix(&mut socket, "fresh-prefix").await;
            commit_audio(&mut socket, "fresh-audio").await;
            let request = expect_event(&mut socket, "response.create").await;
            assert_eq!(request["response"]["input"][0]["id"], "fresh-prefix");
            assert_eq!(request["response"]["input"][1]["id"], "fresh-audio");
            complete_response(&mut socket, &request, "fresh-response", RESULT).await;
            delete_audio(&mut socket, "fresh-audio").await;
            delete_audio(&mut socket, "fresh-prefix").await;
        });
        let memory = Memory::frozen(Default::default());
        let input = memory.prompt_input(&memory.stable_prefix(), 0);
        let mut connection = Connection::connect(&settings, None).await.unwrap();
        if at_commit {
            connection.begin(&input.stable, "cancelled").await.unwrap();
            connection.append(&[1; 2400]).await.unwrap();
            tokio::select! {
                result = connection.finish(&input, "cancelled") => panic!("unexpected {result:?}"),
                _ = waiting => {}
            }
        } else {
            tokio::select! {
                result = connection.begin(&input.stable, "cancelled") => panic!("unexpected {result:?}"),
                _ = waiting => {}
            }
        }
        release.send(()).unwrap();
        connection.cancel().await.unwrap();
        assert!(connection.append(&[1; 2400]).await.is_err());
        connection.begin(&input.stable, "fresh").await.unwrap();
        connection.append(&[1; 2400]).await.unwrap();
        assert_eq!(
            connection.finish(&input, "fresh").await.unwrap().text,
            "测试"
        );
        connection.delete_segment().await.unwrap();
        server.await.unwrap();
    }
}
