use super::*;

// ---------------------------------------------------------------------------
// Layer 1: direct Claude proxy (preserved verbatim from the original module).
// ---------------------------------------------------------------------------

pub(crate) async fn run_turn(inner: Arc<AgentInner>) {
    let Some(api_key) = inner.api_key.clone() else {
        return;
    };

    let history = inner.history.lock().clone();
    let messages: Vec<serde_json::Value> = history
        .iter()
        .map(|t| json!({ "role": t.role, "content": t.content }))
        .collect();

    let mut body = json!({
        "messages": messages,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "stream": true,
    });
    if !inner.model.is_empty() {
        body["model"] = json!(inner.model);
    } else {
        body["model"] = json!("claude-opus-4-7");
    }

    let request = inner
        .http
        .post(ANTHROPIC_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body);

    let resp = match request.send().await {
        Ok(r) => r,
        Err(err) => {
            let _ = inner.tx.send(AgentServerMessage::Error {
                message: format!("network: {err}"),
            });
            return;
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        let _ = inner.tx.send(AgentServerMessage::Error {
            message: format!("anthropic {status}: {body_text}"),
        });
        return;
    }

    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut assistant_accum = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(b) => b,
            Err(err) => {
                let _ = inner.tx.send(AgentServerMessage::Error {
                    message: format!("stream: {err}"),
                });
                break;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find("\n\n") {
            let record = buf[..idx].to_string();
            buf.drain(..idx + 2);
            for line in record.lines() {
                if let Some(payload) = line.strip_prefix("data: ") {
                    if payload == "[DONE]" {
                        continue;
                    }
                    forward_sse_event(
                        &inner.tx,
                        "direct-proxy",
                        payload,
                        &mut assistant_accum,
                    );
                }
            }
        }
    }

    if !assistant_accum.is_empty() {
        inner.history.lock().push(HistoryTurn {
            role: "assistant",
            content: assistant_accum,
        });
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: SseMessageStart },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: SseContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: SseContentDelta },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: SseMessageDelta },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SseMessageStart {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum SseContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum SseContentDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SseMessageDelta {
    #[serde(default)]
    stop_reason: Option<String>,
}

pub(crate) type BlockKinds = std::collections::HashMap<u32, BlockKindEntry>;

#[derive(Clone)]
pub(crate) struct BlockKindEntry {
    kind: ContentKind,
}

thread_local! {
    static BLOCK_KINDS: std::cell::RefCell<BlockKinds> =
        std::cell::RefCell::new(BlockKinds::new());
    static CURRENT_MESSAGE_ID: std::cell::RefCell<String> =
        const { std::cell::RefCell::new(String::new()) };
}

pub(crate) fn forward_sse_event(
    tx: &UnboundedSender<AgentServerMessage>,
    session_id: &str,
    payload: &str,
    accum: &mut String,
) {
    let ev: SseEvent = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return,
    };
    match ev {
        SseEvent::MessageStart { message } => {
            BLOCK_KINDS.with(|m| m.borrow_mut().clear());
            CURRENT_MESSAGE_ID.with(|m| *m.borrow_mut() = message.id.clone());
            let _ = tx.send(AgentServerMessage::MessageStart {
                session_id: session_id.to_string(),
                role: Role::Assistant,
                message_id: message.id,
            });
        }
        SseEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            let (kind, seed_text) = match content_block {
                SseContentBlock::Text { text } => (ContentKind::Text, text),
                SseContentBlock::Thinking { thinking } => {
                    (ContentKind::Reasoning, thinking)
                }
                SseContentBlock::ToolUse { name, input } => (
                    ContentKind::Tool { name },
                    if input.is_null() {
                        String::new()
                    } else {
                        input.to_string()
                    },
                ),
                SseContentBlock::Other => return,
            };
            BLOCK_KINDS.with(|m| {
                m.borrow_mut()
                    .insert(index, BlockKindEntry { kind: kind.clone() });
            });
            let message_id = CURRENT_MESSAGE_ID.with(|m| m.borrow().clone());
            if !seed_text.is_empty() {
                if matches!(kind, ContentKind::Text) {
                    accum.push_str(&seed_text);
                }
                let _ = tx.send(AgentServerMessage::ContentDelta {
                    session_id: session_id.to_string(),
                    message_id,
                    kind,
                    text: seed_text,
                });
            }
        }
        SseEvent::ContentBlockDelta { index, delta } => {
            let kind =
                BLOCK_KINDS.with(|m| m.borrow().get(&index).map(|e| e.kind.clone()));
            let Some(kind) = kind else { return };
            let text = match delta {
                SseContentDelta::Text { text } => text,
                SseContentDelta::Thinking { thinking } => thinking,
                SseContentDelta::InputJson { partial_json } => partial_json,
                SseContentDelta::Other => return,
            };
            if matches!(kind, ContentKind::Text) {
                accum.push_str(&text);
            }
            let message_id = CURRENT_MESSAGE_ID.with(|m| m.borrow().clone());
            let _ = tx.send(AgentServerMessage::ContentDelta {
                session_id: session_id.to_string(),
                message_id,
                kind,
                text,
            });
        }
        SseEvent::MessageDelta { delta } => {
            if let Some(reason) = delta.stop_reason {
                let message_id = CURRENT_MESSAGE_ID.with(|m| m.borrow().clone());
                let _ = tx.send(AgentServerMessage::MessageEnd {
                    session_id: session_id.to_string(),
                    message_id,
                    stop_reason: reason,
                });
            }
        }
        SseEvent::MessageStop => {
            let message_id = CURRENT_MESSAGE_ID.with(|m| m.borrow().clone());
            if !message_id.is_empty() {
                let _ = tx.send(AgentServerMessage::MessageEnd {
                    session_id: session_id.to_string(),
                    message_id,
                    stop_reason: "end_turn".to_string(),
                });
            }
        }
        SseEvent::Other => {}
    }
}

// ---------------------------------------------------------------------------
// Layer 2: agent-server HTTP/SSE proxy.
// ---------------------------------------------------------------------------

pub(crate) fn emit_error(
    tx: &UnboundedSender<AgentServerMessage>,
    message: impl Into<String>,
) {
    let _ = tx.send(AgentServerMessage::Error {
        message: message.into(),
    });
}
