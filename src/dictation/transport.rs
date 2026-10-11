use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

use super::{config::DictationConfig, memory::Snapshot, result::DictationResult};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(90);

pub(super) struct Connection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    instructions: String,
    response_id: Option<String>,
    audio_id: Option<String>,
    commit_pending: bool,
    response_pending: Option<String>,
    pending_events: VecDeque<Value>,
}

impl Connection {
    pub async fn connect(settings: &DictationConfig, key: Option<&str>) -> Result<Self> {
        settings.validate()?;
        let mut url = reqwest::Url::parse(&settings.url)?;
        url.query_pairs_mut().append_pair("model", &settings.model);
        let mut request = url.as_str().into_client_request()?;
        if let Some(key) = key.filter(|key| !key.is_empty()) {
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {key}")
                    .parse()
                    .context("invalid Realtime API key")?,
            );
        }
        let limits = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024))
            .max_frame_size(Some(256 * 1024));
        let (socket, _) = timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async_with_config(request, Some(limits), true),
        )
        .await
        .context("Realtime connection timeout")?
        .map_err(|_| {
            anyhow::anyhow!("Realtime WebSocket handshake failed (check endpoint/authentication)")
        })?;
        let mut connection = Self {
            socket,
            instructions: settings.prompt(),
            response_id: None,
            audio_id: None,
            commit_pending: false,
            response_pending: None,
            pending_events: VecDeque::new(),
        };
        timeout(Duration::from_secs(10), async {
            connection.wait_type("session.created").await?;
            connection.send(json!({"type":"session.update", "session":{
                "type":"realtime", "model":settings.model, "output_modalities":["text"],
                "instructions":connection.instructions, "max_output_tokens":4096,
                "audio":{"input":{"format":{"type":"audio/pcm","rate":24000},"turn_detection":null}}
            }})).await?;
            connection.wait_type("session.updated").await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Realtime session configuration timeout")??;
        Ok(connection)
    }

    async fn send(&mut self, event: Value) -> Result<()> {
        timeout(
            Duration::from_secs(10),
            self.socket.send(Message::text(event.to_string())),
        )
        .await
        .context("Realtime send timeout")??;
        Ok(())
    }

    async fn receive(&mut self) -> Result<Value> {
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(event);
        }
        self.read_event().await
    }

    /// Keep control traffic flowing while waiting for the next microphone frame.
    /// Protocol events remain ordered for the response/commit consumer.
    pub async fn receive_while_recording(&mut self) -> Result<()> {
        let event = self.read_event().await?;
        ensure!(
            self.pending_events.len() < 32,
            "Realtime pending event capacity exceeded"
        );
        self.pending_events.push_back(event);
        Ok(())
    }

    async fn read_event(&mut self) -> Result<Value> {
        loop {
            match self
                .socket
                .next()
                .await
                .context("Realtime connection closed")??
            {
                Message::Text(text) => {
                    let event: Value =
                        serde_json::from_str(&text).context("invalid Realtime event JSON")?;
                    if event["type"] == "error" {
                        // Server messages may echo user audio/text or credentials. Log only a bounded code.
                        let code: String = event["error"]["code"]
                            .as_str()
                            .unwrap_or("unknown")
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .take(80)
                            .collect();
                        bail!(
                            "Realtime server rejected an operation ({code}); check protocol compatibility"
                        );
                    }
                    return Ok(event);
                }
                Message::Close(_) => bail!("Realtime connection closed"),
                Message::Ping(_) => self.socket.flush().await?,
                Message::Pong(_) => {}
                _ => bail!("unsupported Realtime binary event"),
            }
        }
    }

    async fn wait_type(&mut self, kind: &str) -> Result<Value> {
        loop {
            let event = self.receive().await?;
            if event["type"] == kind {
                return Ok(event);
            }
        }
    }

    /// Polls idle sockets so closes/expiry are noticed before a subsequent recording.
    pub async fn idle(&mut self) -> Result<()> {
        loop {
            match timeout(Duration::from_secs(25), self.receive()).await {
                Ok(result) => {
                    result?;
                }
                Err(_) => self.socket.send(Message::Ping(Vec::new().into())).await?,
            }
        }
    }

    pub async fn append(&mut self, pcm: &[i16]) -> Result<()> {
        for frame in pcm.chunks(4800) {
            let bytes: Vec<_> = frame.iter().flat_map(|s| s.to_le_bytes()).collect();
            self.send(json!({"type":"input_audio_buffer.append","audio":STANDARD.encode(bytes)}))
                .await?;
        }
        Ok(())
    }

    pub async fn clear(&mut self) -> Result<()> {
        timeout(Duration::from_secs(10), async {
            self.send(json!({"type":"input_audio_buffer.clear"}))
                .await?;
            self.wait_type("input_audio_buffer.cleared").await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Realtime clear timeout")?
    }

    pub async fn cancel(&mut self) -> Result<()> {
        timeout(Duration::from_secs(10), async {
            if self.commit_pending {
                let event = self.wait_type("input_audio_buffer.committed").await?;
                self.audio_id = Some(
                    event["item_id"]
                        .as_str()
                        .context("missing committed item")?
                        .to_owned(),
                );
                self.commit_pending = false;
            }
            if let Some(correlation) = self.response_pending.take() {
                let event = self.wait_type("response.created").await?;
                ensure!(
                    event["response"]["metadata"]["dictation_id"] == correlation,
                    "unexpected response during cancellation"
                );
                self.response_id = Some(
                    event["response"]["id"]
                        .as_str()
                        .context("missing response id")?
                        .to_owned(),
                );
            }
            if let Some(id) = self.response_id.take() {
                self.send(json!({"type":"response.cancel","response_id":id}))
                    .await?;
                loop {
                    let event = self.wait_type("response.done").await?;
                    if event["response"]["id"] == id {
                        break;
                    }
                }
            }
            if self.audio_id.is_some() {
                self.delete_audio().await?;
            }
            self.clear().await
        })
        .await
        .context("Realtime cancellation timeout")?
    }

    /// Delete the completed input item. Callers retain validated text before awaiting cleanup.
    pub async fn delete_audio(&mut self) -> Result<()> {
        let audio_id = self
            .audio_id
            .clone()
            .context("no Realtime audio item to delete")?;
        timeout(Duration::from_secs(10), async {
            self.send(json!({"type":"conversation.item.delete","item_id":audio_id}))
                .await?;
            loop {
                let event = self.wait_type("conversation.item.deleted").await?;
                if event["item_id"] == audio_id {
                    break;
                }
            }
            self.audio_id = None;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Realtime audio cleanup timeout")?
    }

    /// Return a validated response independently of audio cleanup (see `delete_audio`).
    /// Each response sees an explicit snapshot and one audio item, independent of connection history.
    pub async fn finish(
        &mut self,
        snapshot: &Snapshot,
        correlation: &str,
    ) -> Result<DictationResult> {
        timeout(RESPONSE_TIMEOUT, self.finish_inner(snapshot, correlation))
            .await
            .context("Realtime response timeout")?
    }

    async fn finish_inner(
        &mut self,
        snapshot: &Snapshot,
        correlation: &str,
    ) -> Result<DictationResult> {
        self.commit_pending = true;
        self.send(json!({"type":"input_audio_buffer.commit"}))
            .await?;
        let committed = self.wait_type("input_audio_buffer.committed").await?;
        let audio_id = committed["item_id"]
            .as_str()
            .context("missing committed audio item")?
            .to_owned();
        self.audio_id = Some(audio_id.clone());
        self.commit_pending = false;
        self.response_pending = Some(correlation.to_owned());
        self.send(json!({"type":"response.create", "response":{
            "conversation":"none", "output_modalities":["text"], "instructions":self.instructions,
            "max_output_tokens":4096, "metadata":{"dictation_id":correlation},
            "input":[{"type":"message","role":"user","content":[{"type":"input_text",
                "text":serde_json::to_string(snapshot)?}]},
                {"type":"item_reference","id":audio_id}]
        }}))
        .await?;
        let mut response_id = None;
        let mut seen = HashSet::new();
        let mut delta = String::new();
        let mut done_text = None;
        let complete = loop {
            let event = self.receive().await?;
            if let Some(id) = event["event_id"].as_str() {
                ensure!(seen.len() < 20_000, "too many Realtime events");
                if !seen.insert(id.to_owned()) {
                    continue;
                }
            }
            match event["type"].as_str() {
                Some("response.created") => {
                    ensure!(
                        event["response"]["metadata"]["dictation_id"] == correlation,
                        "unexpected Realtime response correlation"
                    );
                    ensure!(
                        response_id.is_none(),
                        "duplicate Realtime response creation"
                    );
                    response_id = Some(
                        event["response"]["id"]
                            .as_str()
                            .context("missing response id")?
                            .to_owned(),
                    );
                    self.response_id.clone_from(&response_id);
                    self.response_pending = None;
                }
                Some("response.output_text.delta" | "response.output_text.done") => {
                    ensure!(
                        response_id.as_deref() == event["response_id"].as_str()
                            && response_id.is_some(),
                        "unexpected Realtime text response"
                    );
                    ensure!(
                        event["output_index"] == 0 && event["content_index"] == 0,
                        "unexpected Realtime content part"
                    );
                    if event["type"] == "response.output_text.delta" {
                        delta.push_str(event["delta"].as_str().context("missing text delta")?);
                        ensure!(delta.len() <= 64 * 1024, "Realtime response too large");
                    } else {
                        let text = event["text"].as_str().context("missing final text")?;
                        ensure!(
                            delta.is_empty() || delta == text,
                            "Realtime delta/final mismatch"
                        );
                        done_text = Some(text.to_owned());
                    }
                }
                Some("response.done") => {
                    ensure!(
                        response_id.is_some()
                            && response_id.as_deref() == event["response"]["id"].as_str(),
                        "unexpected Realtime completion"
                    );
                    ensure!(
                        event["response"]["status"] == "completed",
                        "Realtime response failed, cancelled or truncated"
                    );
                    let output = event["response"]["output"]
                        .as_array()
                        .context("missing response output")?;
                    ensure!(output.len() == 1, "expected one dictation output");
                    let content = output[0]["content"]
                        .as_array()
                        .context("missing output content")?;
                    ensure!(
                        content.len() == 1 && content[0]["type"] == "output_text",
                        "expected one text part"
                    );
                    let text = content[0]["text"]
                        .as_str()
                        .context("missing completed text")?;
                    ensure!(
                        done_text.as_deref() == Some(text),
                        "Realtime completion/text mismatch"
                    );
                    self.response_id = None;
                    break text.to_owned();
                }
                _ => {}
            }
        };
        DictationResult::parse(&complete, &snapshot.ids())
    }
}

#[cfg(test)]
mod tests;
