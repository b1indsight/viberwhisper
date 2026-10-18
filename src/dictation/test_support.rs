//! Fixed protocol steps shared by local WebSocket tests.
use super::config::DictationConfig;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

pub(super) async fn accept(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    send_json(&mut socket, json!({"type":"session.created"})).await;
    socket
}

pub(super) async fn send_json(socket: &mut WebSocketStream<TcpStream>, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

/// Skip uploads when expecting a control event, preserving control-event order.
pub(super) async fn expect_event(socket: &mut WebSocketStream<TcpStream>, kind: &str) -> Value {
    loop {
        let message = socket.next().await.expect("client disconnected").unwrap();
        let Message::Text(text) = message else {
            panic!("expected JSON event, got {message:?}");
        };
        let event: Value = serde_json::from_str(&text).unwrap();
        if event["type"] == "input_audio_buffer.append" && kind != "input_audio_buffer.append" {
            continue;
        }
        assert_eq!(event["type"], kind);
        return event;
    }
}

/// Bind an isolated local service and point the client configuration at it.
pub(super) async fn listen() -> (TcpListener, DictationConfig) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let settings = DictationConfig {
        url: format!("ws://{}/v1/realtime", listener.local_addr().unwrap()),
        ..Default::default()
    };
    (listener, settings)
}

pub(super) async fn accept_session(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let mut socket = accept(listener).await;
    expect_event(&mut socket, "session.update").await;
    send_json(&mut socket, json!({"type":"session.updated"})).await;
    socket
}

pub(super) async fn declare_prefix(socket: &mut WebSocketStream<TcpStream>, id: &str) -> Value {
    let declared = expect_event(socket, "conversation.item.create").await;
    let mut item = declared["item"].clone();
    item["id"] = json!(id);
    item["status"] = json!("completed");
    for kind in ["conversation.item.added", "conversation.item.done"] {
        send_json(socket, json!({"type":kind,"item":item})).await;
    }
    declared
}

pub(super) async fn commit_segment(socket: &mut WebSocketStream<TcpStream>, id: &str) -> Value {
    let prefix = declare_prefix(socket, &format!("prefix-{id}")).await;
    commit_audio(socket, id).await;
    prefix
}

pub(super) async fn commit_audio(socket: &mut WebSocketStream<TcpStream>, id: &str) {
    expect_event(socket, "input_audio_buffer.commit").await;
    send_json(
        socket,
        json!({"type":"input_audio_buffer.committed","item_id":id}),
    )
    .await;
}

pub(super) async fn delete_audio(socket: &mut WebSocketStream<TcpStream>, id: &str) {
    let event = expect_event(socket, "conversation.item.delete").await;
    assert_eq!(event["item_id"], id);
    send_json(
        socket,
        json!({"type":"conversation.item.deleted","item_id":event["item_id"]}),
    )
    .await;
}

pub(super) async fn delete_segment(socket: &mut WebSocketStream<TcpStream>, id: &str) {
    delete_audio(socket, id).await;
    delete_audio(socket, &format!("prefix-{id}")).await;
}

/// Complete one response with correlated, consistent final-text events.
pub(super) async fn complete_response(
    socket: &mut WebSocketStream<TcpStream>,
    request: &Value,
    id: &str,
    text: &str,
) {
    for reply in [
        json!({"type":"response.created","response":{"id":id,"metadata":request["response"]["metadata"]}}),
        json!({"type":"response.output_text.done","response_id":id,"output_index":0,"content_index":0,"text":text}),
        json!({"type":"response.done","response":{"id":id,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}),
    ] {
        send_json(socket, reply).await;
    }
}
