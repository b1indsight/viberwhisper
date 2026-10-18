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

use super::{
    config::DictationConfig,
    memory::{PromptInput, StablePrefix},
    result::DictationResult,
};

struct Segment {
    correlation: String,
    prefix_text: String,
    prefix: PrefixState,
    audio: AudioState,
}

enum PrefixState {
    Pending { added_id: Option<String> },
    Ready(String),
    Deleting(String),
}

enum AudioState {
    Recording,
    Committing,
    Committed(String),
    Deleting(String),
    Deleted,
}

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(90);

pub(super) struct Connection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    instructions: String,
    response_id: Option<String>,
    segment: Option<Segment>,
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
            segment: None,
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

    /// Declare the exact model prefix before uploading any part of this segment.
    pub async fn begin(&mut self, stable: &StablePrefix, correlation: &str) -> Result<()> {
        ensure!(
            self.segment.is_none(),
            "previous Realtime segment has not been cleaned up"
        );
        let text = serde_json::to_string(stable)?;
        self.segment = Some(Segment {
            correlation: correlation.to_owned(),
            prefix_text: text.clone(),
            prefix: PrefixState::Pending { added_id: None },
            audio: AudioState::Recording,
        });
        timeout(Duration::from_secs(10), async {
            self.send(json!({"type":"conversation.item.create", "item":{
                "type":"message", "role":"user", "content":[{"type":"input_text", "text":text}]
            }}))
            .await?;
            self.confirm_prefix().await
        })
        .await
        .context("Realtime prefix declaration timeout")?
    }

    /// Bind server-assigned IDs only after validating the complete echoed text item.
    /// The created event supports providers using the earlier single-ack protocol.
    async fn confirm_prefix(&mut self) -> Result<()> {
        while matches!(
            self.segment.as_ref().map(|s| &s.prefix),
            Some(PrefixState::Pending { .. })
        ) {
            let event = self.receive().await?;
            let kind = event["type"].as_str().unwrap_or_default();
            if !matches!(
                kind,
                "conversation.item.added" | "conversation.item.done" | "conversation.item.created"
            ) {
                continue;
            }
            let item = &event["item"];
            if item["role"] != "user" {
                continue;
            }
            let segment = self.segment.as_mut().unwrap();
            ensure!(
                item["type"] == "message"
                    && item["content"].as_array().is_some_and(|c| c.len() == 1)
                    && item["content"][0]["type"] == "input_text"
                    && item["content"][0]["text"] == segment.prefix_text,
                "Realtime prefix acknowledgement differs from declared content"
            );
            let id = item["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .context("missing Realtime prefix item id")?
                .to_owned();
            let PrefixState::Pending { added_id } = &mut segment.prefix else {
                unreachable!()
            };
            if kind == "conversation.item.added" {
                ensure!(added_id.is_none(), "duplicate Realtime prefix item");
                *added_id = Some(id);
            } else {
                ensure!(
                    item["status"] == "completed",
                    "Realtime prefix item is not completed"
                );
                ensure!(
                    added_id.as_ref() == Some(&id)
                        || (kind == "conversation.item.created" && added_id.is_none()),
                    "unexpected Realtime prefix completion"
                );
                segment.prefix = PrefixState::Ready(id);
            }
        }
        Ok(())
    }

    pub async fn append(&mut self, pcm: &[i16]) -> Result<()> {
        ensure!(
            self.segment
                .as_ref()
                .is_some_and(|s| matches!(s.prefix, PrefixState::Ready(_))
                    && matches!(s.audio, AudioState::Recording)),
            "Realtime audio requires an acknowledged prefix"
        );
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
            self.delete_segment().await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Realtime clear timeout")?
    }

    pub async fn cancel(&mut self) -> Result<()> {
        timeout(Duration::from_secs(10), async {
            self.confirm_prefix().await?;
            if self
                .segment
                .as_ref()
                .is_some_and(|s| matches!(s.audio, AudioState::Committing))
            {
                let event = self.wait_type("input_audio_buffer.committed").await?;
                self.segment.as_mut().unwrap().audio = AudioState::Committed(
                    event["item_id"]
                        .as_str()
                        .context("missing committed item")?
                        .to_owned(),
                );
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
            self.delete_segment().await?;
            self.clear().await
        })
        .await
        .context("Realtime cancellation timeout")?
    }

    /// Delete this segment's audio and text, retaining ownership until both are acknowledged.
    /// Callers retain validated text before awaiting this fallible cleanup.
    pub async fn delete_segment(&mut self) -> Result<()> {
        timeout(Duration::from_secs(10), async {
            let Some(segment) = &self.segment else {
                return Ok(());
            };
            ensure!(
                !matches!(segment.prefix, PrefixState::Pending { .. }),
                "cannot clean up an unacknowledged Realtime prefix"
            );
            ensure!(
                !matches!(segment.audio, AudioState::Committing),
                "cannot clean up an unacknowledged Realtime audio commit"
            );
            let audio = match &segment.audio {
                AudioState::Committed(id) => Some((id.clone(), false)),
                AudioState::Deleting(id) => Some((id.clone(), true)),
                _ => None,
            };
            if let Some((id, pending)) = audio {
                self.segment.as_mut().unwrap().audio = AudioState::Deleting(id.clone());
                self.delete_item(&id, pending).await?;
                self.segment.as_mut().unwrap().audio = AudioState::Deleted;
            }
            let (id, pending) = match &self.segment.as_ref().unwrap().prefix {
                PrefixState::Ready(id) => (id.clone(), false),
                PrefixState::Deleting(id) => (id.clone(), true),
                PrefixState::Pending { .. } => unreachable!(),
            };
            self.segment.as_mut().unwrap().prefix = PrefixState::Deleting(id.clone());
            self.delete_item(&id, pending).await?;
            self.segment = None;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Realtime segment cleanup timeout")?
    }

    /// Resume an interrupted deletion by reading its acknowledgement, never resending its ID.
    async fn delete_item(&mut self, id: &str, pending: bool) -> Result<()> {
        if !pending {
            self.send(json!({"type":"conversation.item.delete","item_id":id}))
                .await?;
        }
        loop {
            let event = self.wait_type("conversation.item.deleted").await?;
            if event["item_id"] == id {
                return Ok(());
            }
        }
    }

    /// Return a validated response independently of item cleanup (see `delete_segment`).
    /// Model input is the accepted prefix, this segment's audio, then its dynamic context.
    pub async fn finish(
        &mut self,
        input: &PromptInput,
        correlation: &str,
    ) -> Result<DictationResult> {
        timeout(RESPONSE_TIMEOUT, self.finish_inner(input, correlation))
            .await
            .context("Realtime response timeout")?
    }

    async fn finish_inner(
        &mut self,
        input: &PromptInput,
        correlation: &str,
    ) -> Result<DictationResult> {
        let segment = self
            .segment
            .as_mut()
            .context("no declared Realtime segment")?;
        ensure!(
            segment.correlation == correlation
                && segment.prefix_text == serde_json::to_string(&input.stable)?,
            "Realtime response does not match the declared segment prefix"
        );
        let PrefixState::Ready(prefix_id) = &segment.prefix else {
            bail!("Realtime prefix has not been acknowledged");
        };
        let prefix_id = prefix_id.clone();
        ensure!(
            matches!(segment.audio, AudioState::Recording),
            "Realtime audio was already committed"
        );
        segment.audio = AudioState::Committing;
        self.send(json!({"type":"input_audio_buffer.commit"}))
            .await?;
        let committed = self.wait_type("input_audio_buffer.committed").await?;
        let audio_id = committed["item_id"]
            .as_str()
            .context("missing committed audio item")?
            .to_owned();
        self.segment.as_mut().unwrap().audio = AudioState::Committed(audio_id.clone());
        self.response_pending = Some(correlation.to_owned());
        self.send(json!({"type":"response.create", "response":{
            "conversation":"none", "output_modalities":["text"], "instructions":self.instructions,
            "max_output_tokens":4096, "metadata":{"dictation_id":correlation},
            "input":[{"type":"item_reference","id":prefix_id},
                {"type":"item_reference","id":audio_id},
                {"type":"message","role":"user","content":[{"type":"input_text",
                    "text":serde_json::to_string(&input.dynamic)?}]}]
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
        DictationResult::parse(&complete, &input.snapshot().ids())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod prefix_tests;
