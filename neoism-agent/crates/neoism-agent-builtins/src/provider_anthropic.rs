use std::collections::BTreeMap;

use anyhow::Context;
use neoism_agent_core::{
    AuthInfo, ProviderGenerationRequest, ProviderMessage, ProviderRole,
    ProviderStreamEvent, ProviderToolCall, ToolListItem,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::provider_error::ProviderError;

use super::provider_openai_stream::{estimate_tokens, neoism_user_agent};
use super::{ProviderEventStream, ProviderRuntime};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const CLAUDE_CODE_PROVIDER_ID: &str = "claude-code";

#[derive(Clone)]
pub(super) struct AnthropicClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Clone)]
pub(super) struct AnthropicRuntime {
    pub(super) client: AnthropicClient,
    pub(super) auth: Option<AuthInfo>,
}

impl AnthropicClient {
    pub(super) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

impl ProviderRuntime for AnthropicRuntime {
    fn stream(&self, request: ProviderGenerationRequest) -> ProviderEventStream {
        let request = crate::provider_transform::normalize_request(request);
        let client = self.client.clone();
        let auth = self.auth.clone();
        Box::pin(async_stream::try_stream! {
            let api_key = anthropic_key(&request.provider_id, auth.as_ref()).ok_or_else(|| {
                anyhow::anyhow!(
                    "Anthropic provider requested but no API key was found in stored auth or provider environment variables"
                )
            })?;
            tracing::info!(
                provider = %request.provider_id,
                model = %request.model_id,
                api_model = request.api.as_ref().map(|api| api.id.as_str()).unwrap_or(&request.model_id),
                base_url = %client.base_url,
                auth_source = %anthropic_auth_source(&request.provider_id, auth.as_ref()),
                "anthropic-format provider request"
            );

            yield ProviderStreamEvent::Start;
            yield ProviderStreamEvent::StartStep;

            let mut body = anthropic_body(&request);
            merge_provider_options(&mut body, &request.options);

            let mut http = client
                .client
                .post(format!("{}/messages", client.base_url))
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("user-agent", neoism_user_agent())
                .json(&body);
            for (name, value) in &request.headers {
                http = http.header(name, value);
            }
            if request.provider_id == CLAUDE_CODE_PROVIDER_ID {
                http = http
                    .header("x-meridian-source", "neoism")
                    .header("x-meridian-agent", "neoism")
                    .header("x-meridian-client", "neoism");
                if let Some(session_id) = request.session_id.as_deref().filter(|id| !id.is_empty()) {
                    http = http.header("x-meridian-session-id", session_id);
                }
            }

            let response = http
                .send()
                .await
                .context("failed to send Anthropic streaming messages request")?;
            let status = response.status();
            let headers = response.headers().clone();
            let mut response = if status.is_success() {
                response
            } else {
                let body = response.text().await.unwrap_or_default();
                Err::<reqwest::Response, anyhow::Error>(
                    ProviderError::from_response("Anthropic", status, &headers, body).into(),
                )?
            };

            let mut parser = AnthropicSseParser::default();
            let mut line = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                for byte in chunk {
                    if byte == b'\n' {
                        let line_text = std::str::from_utf8(&line)?.to_string();
                        line.clear();
                        for event in parser.push_line(&line_text)? {
                            yield event;
                        }
                    } else {
                        line.push(byte);
                    }
                }
            }
            if !line.is_empty() {
                let line_text = std::str::from_utf8(&line)?;
                for event in parser.push_line(line_text)? {
                    yield event;
                }
            }
            for event in parser.finish()? {
                yield event;
            }
        })
    }
}

fn anthropic_key(provider_id: &str, auth: Option<&AuthInfo>) -> Option<String> {
    if provider_id == CLAUDE_CODE_PROVIDER_ID {
        return Some("dummy".to_string());
    }
    match auth {
        Some(AuthInfo::Api { key, .. }) => Some(key.clone()),
        Some(AuthInfo::OAuth { access, .. }) => Some(access.clone()),
        _ => std::env::var("ANTHROPIC_API_KEY").ok(),
    }
}

fn anthropic_auth_source(provider_id: &str, auth: Option<&AuthInfo>) -> &'static str {
    if provider_id == CLAUDE_CODE_PROVIDER_ID {
        return "claude-code-local-proxy";
    }
    match auth {
        Some(AuthInfo::Api { .. }) => "stored-api-key",
        Some(AuthInfo::OAuth { .. }) => "stored-oauth-token",
        _ if std::env::var_os("ANTHROPIC_API_KEY").is_some() => "env-api-key",
        _ => "missing",
    }
}

fn anthropic_body(request: &ProviderGenerationRequest) -> Value {
    let system = request
        .messages
        .iter()
        .filter(|message| matches!(message.role, ProviderRole::System))
        .filter_map(|message| non_empty_text(&message.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    let messages = request
        .messages
        .iter()
        .filter(|message| !matches!(message.role, ProviderRole::System))
        .map(anthropic_message)
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": request.api.as_ref().map(|api| api.id.as_str()).unwrap_or(&request.model_id),
        "max_tokens": request
            .options
            .get("max_tokens")
            .or_else(|| request.options.get("maxTokens"))
            .and_then(Value::as_u64)
            .unwrap_or(4096),
        "messages": messages,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = Value::String(system);
    }
    let tools = anthropic_tools(&request.model_id, &request.tools);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = json!({ "type": "auto" });
    }
    crate::provider_transform::apply_anthropic_request_quirks(request, &mut body);
    body
}

fn anthropic_message(message: &ProviderMessage) -> Value {
    match message.role {
        ProviderRole::User => json!({
            "role": "user",
            "content": anthropic_user_content(message),
        }),
        ProviderRole::Assistant => json!({
            "role": "assistant",
            "content": anthropic_assistant_content(message),
        }),
        ProviderRole::Tool => json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                "content": message.content,
                "is_error": message.tool_error.unwrap_or(false),
            }],
        }),
        ProviderRole::System => {
            unreachable!("system messages are filtered before Anthropic conversion")
        }
    }
}

fn anthropic_user_content(message: &ProviderMessage) -> Value {
    let mut blocks = Vec::new();
    if let Some(text) = non_empty_text(&message.content) {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    for attachment in &message.attachments {
        if !attachment.mime.starts_with("image/") {
            blocks.push(json!({
                "type": "text",
                "text": format!(
                    "[Unsupported attachment omitted: {}{}]",
                    attachment.mime,
                    attachment
                        .filename
                        .as_deref()
                        .map(|name| format!(" {name}"))
                        .unwrap_or_default()
                )
            }));
            continue;
        }
        if let Some((media_type, data)) = data_url_image(&attachment.url) {
            blocks.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                }
            }));
        } else {
            blocks.push(json!({
                "type": "text",
                "text": format!("[Image attachment URL omitted for Anthropic: {}]", attachment.url),
            }));
        }
    }
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "." }));
    }
    Value::Array(blocks)
}

fn anthropic_assistant_content(message: &ProviderMessage) -> Value {
    let mut blocks = Vec::new();
    if let Some(text) = non_empty_text(&message.content) {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    blocks.extend(message.tool_calls.iter().map(anthropic_tool_use));
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "." }));
    }
    Value::Array(blocks)
}

fn anthropic_tool_use(call: &ProviderToolCall) -> Value {
    json!({
        "type": "tool_use",
        "id": call.id,
        "name": call.name,
        "input": call.input,
    })
}

fn anthropic_tools(model_id: &str, tools: &[ToolListItem]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.id,
                "description": tool.description,
                "input_schema": crate::provider_transform::tool_parameters(model_id, &tool.parameters),
            })
        })
        .collect()
}

fn non_empty_text(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn data_url_image(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    media_type
        .starts_with("image/")
        .then_some((media_type, data))
}

fn merge_provider_options(body: &mut Value, options: &BTreeMap<String, Value>) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    for (key, value) in options {
        if matches!(
            key.as_str(),
            "model" | "messages" | "system" | "stream" | "tools" | "tool_choice"
        ) {
            continue;
        }
        let key = if key == "maxTokens" {
            "max_tokens".to_string()
        } else {
            key.clone()
        };
        object.insert(key, value.clone());
    }
}

#[derive(Default)]
struct AnthropicSseParser {
    blocks: BTreeMap<u64, AnthropicBlock>,
    text_started: bool,
    reasoning_started: bool,
    output_text: String,
    finish: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

#[derive(Debug)]
struct AnthropicBlock {
    id: String,
    name: String,
    input: String,
    kind: AnthropicBlockKind,
    started: bool,
}

#[derive(Debug, Eq, PartialEq)]
enum AnthropicBlockKind {
    Text,
    Reasoning,
    Tool,
}

impl AnthropicSseParser {
    fn push_line(&mut self, raw: &str) -> anyhow::Result<Vec<ProviderStreamEvent>> {
        let raw = raw.trim_end_matches('\r').trim();
        let Some(data) = raw.strip_prefix("data:").map(str::trim) else {
            return Ok(Vec::new());
        };
        if data.is_empty() || data == "[DONE]" {
            return Ok(Vec::new());
        }
        let event: AnthropicEvent = serde_json::from_str(data)
            .context("failed to decode Anthropic streaming event")?;
        let mut events = Vec::new();
        match event {
            AnthropicEvent::MessageStart { message } => {
                if let Some(usage) = message.usage {
                    if let Some(tokens) = usage.input_tokens {
                        self.input_tokens = tokens;
                    }
                    if let Some(tokens) = usage.cache_read_input_tokens {
                        self.cache_read_tokens = tokens;
                    }
                    if let Some(tokens) = usage.cache_creation_input_tokens {
                        self.cache_write_tokens = tokens;
                    }
                }
            }
            AnthropicEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                AnthropicContentBlock::Text { text } => {
                    let _ = text;
                    self.blocks.insert(
                        index,
                        AnthropicBlock {
                            id: "text".to_string(),
                            name: String::new(),
                            input: String::new(),
                            kind: AnthropicBlockKind::Text,
                            started: false,
                        },
                    );
                }
                AnthropicContentBlock::Thinking { thinking } => {
                    let _ = thinking;
                    self.blocks.insert(
                        index,
                        AnthropicBlock {
                            id: "reasoning".to_string(),
                            name: String::new(),
                            input: String::new(),
                            kind: AnthropicBlockKind::Reasoning,
                            started: false,
                        },
                    );
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    let input =
                        if input.as_object().is_some_and(|object| object.is_empty()) {
                            String::new()
                        } else {
                            serde_json::to_string(&input).unwrap_or_default()
                        };
                    events.push(ProviderStreamEvent::ToolInputStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    self.blocks.insert(
                        index,
                        AnthropicBlock {
                            id,
                            name,
                            input,
                            kind: AnthropicBlockKind::Tool,
                            started: true,
                        },
                    );
                }
                AnthropicContentBlock::Other => {}
            },
            AnthropicEvent::ContentBlockDelta { index, delta } => {
                let Some(block) = self.blocks.get_mut(&index) else {
                    return Ok(events);
                };
                match delta {
                    AnthropicDelta::TextDelta { text } => {
                        if !self.text_started {
                            events.push(ProviderStreamEvent::TextStart {
                                id: "text".to_string(),
                            });
                            self.text_started = true;
                        }
                        self.output_text.push_str(&text);
                        events.push(ProviderStreamEvent::TextDelta {
                            id: "text".to_string(),
                            delta: text,
                        });
                    }
                    AnthropicDelta::ThinkingDelta { thinking } => {
                        if !self.reasoning_started {
                            events.push(ProviderStreamEvent::ReasoningStart {
                                id: "reasoning".to_string(),
                            });
                            self.reasoning_started = true;
                        }
                        events.push(ProviderStreamEvent::ReasoningDelta {
                            id: "reasoning".to_string(),
                            delta: thinking,
                        });
                    }
                    AnthropicDelta::InputJsonDelta { partial_json } => {
                        block.input.push_str(&partial_json);
                        if !partial_json.is_empty() {
                            events.push(ProviderStreamEvent::ToolInputDelta {
                                id: block.id.clone(),
                                delta: partial_json,
                            });
                        }
                    }
                    AnthropicDelta::Other => {}
                }
            }
            AnthropicEvent::ContentBlockStop { index } => {
                if let Some(block) = self.blocks.remove(&index) {
                    if block.kind == AnthropicBlockKind::Tool && block.started {
                        events.push(ProviderStreamEvent::ToolInputEnd {
                            id: block.id.clone(),
                        });
                        events.push(ProviderStreamEvent::ToolCall {
                            id: block.id,
                            name: block.name,
                            input: parse_tool_input(&block.input),
                        });
                    }
                }
            }
            AnthropicEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.and_then(|delta| delta.stop_reason) {
                    self.finish = Some(reason);
                }
                if let Some(usage) = usage {
                    if let Some(tokens) = usage.output_tokens {
                        self.output_tokens = tokens;
                    }
                    if let Some(tokens) = usage.cache_read_input_tokens {
                        self.cache_read_tokens = tokens;
                    }
                    if let Some(tokens) = usage.cache_creation_input_tokens {
                        self.cache_write_tokens = tokens;
                    }
                }
            }
            AnthropicEvent::MessageStop | AnthropicEvent::Other => {}
        }
        Ok(events)
    }

    fn finish(&mut self) -> anyhow::Result<Vec<ProviderStreamEvent>> {
        let mut events = Vec::new();
        if self.text_started {
            events.push(ProviderStreamEvent::TextEnd {
                id: "text".to_string(),
            });
        }
        if self.reasoning_started {
            events.push(ProviderStreamEvent::ReasoningEnd {
                id: "reasoning".to_string(),
            });
        }
        let output_tokens = if self.output_tokens == 0 {
            estimate_tokens(&self.output_text)
        } else {
            self.output_tokens
        };
        events.push(ProviderStreamEvent::FinishStep {
            finish: self.finish.clone().or_else(|| Some("stop".to_string())),
            total_tokens: None,
            input_tokens: self
                .input_tokens
                .saturating_add(self.cache_read_tokens)
                .saturating_add(self.cache_write_tokens),
            output_tokens,
            reasoning_tokens: 0,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
        });
        events.push(ProviderStreamEvent::Finish {
            finish: self.finish.clone().or_else(|| Some("stop".to_string())),
            total_tokens: None,
            input_tokens: self
                .input_tokens
                .saturating_add(self.cache_read_tokens)
                .saturating_add(self.cache_write_tokens),
            output_tokens,
            reasoning_tokens: 0,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
        });
        Ok(events)
    }
}

fn parse_tool_input(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({}))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicEvent {
    MessageStart {
        message: AnthropicMessageStart,
    },
    ContentBlockStart {
        index: u64,
        content_block: AnthropicContentBlock,
    },
    ContentBlockDelta {
        index: u64,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        index: u64,
    },
    MessageDelta {
        delta: Option<AnthropicMessageDelta>,
        usage: Option<AnthropicUsage>,
    },
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: Option<String>,
    },
    Thinking {
        thinking: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{ProviderAttachment, ProviderToolCall};

    #[test]
    fn anthropic_body_converts_tools_and_media() {
        let mut user = ProviderMessage::text(ProviderRole::User, "inspect");
        user.attachments.push(ProviderAttachment {
            mime: "image/png".to_string(),
            url: "data:image/png;base64,abc".to_string(),
            filename: None,
        });
        let request = ProviderGenerationRequest {
            session_id: None,
            provider_id: "anthropic".to_string(),
            model_id: "claude-sonnet-4".to_string(),
            variant: None,
            text_verbosity: None,
            api: None,
            auth_env: Vec::new(),
            messages: vec![
                ProviderMessage::text(ProviderRole::System, "sys"),
                user,
                ProviderMessage::assistant_tool_call(
                    "",
                    vec![ProviderToolCall {
                        id: "toolu_1".to_string(),
                        name: "read".to_string(),
                        input: json!({ "path": "README.md" }),
                    }],
                ),
                ProviderMessage::tool_result("toolu_1", "read", "ok", false),
            ],
            tools: vec![ToolListItem {
                id: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({ "type": "object" }),
                output_schema: None,
            }],
            options: BTreeMap::new(),
            headers: BTreeMap::new(),
        };

        let body = anthropic_body(&request);

        assert_eq!(body["system"], "sys");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["tools"][0]["name"], "read");
    }

    #[test]
    fn anthropic_body_enables_thinking_for_reasoning_variant() {
        let request = ProviderGenerationRequest {
            session_id: None,
            provider_id: "anthropic".to_string(),
            model_id: "claude-sonnet-4".to_string(),
            variant: Some("high".to_string()),
            text_verbosity: None,
            api: None,
            auth_env: Vec::new(),
            messages: vec![ProviderMessage::text(ProviderRole::User, "think")],
            tools: Vec::new(),
            options: BTreeMap::new(),
            headers: BTreeMap::new(),
        };

        let body = anthropic_body(&request);

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 3072);
    }

    #[test]
    fn anthropic_parser_streams_text_reasoning_and_tools() {
        let mut parser = AnthropicSseParser::default();
        let mut events = Vec::new();
        for line in [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":5}}}"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"TASK.md\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#,
        ] {
            events.extend(parser.push_line(line).unwrap());
        }
        events.extend(parser.finish().unwrap());

        assert!(matches!(events[0], ProviderStreamEvent::TextStart { .. }));
        assert!(events.iter().any(|event| matches!(
            event,
            ProviderStreamEvent::ToolCall { name, input, .. }
                if name == "read" && input["path"] == "TASK.md"
        )));
        assert!(matches!(
            events.last().unwrap(),
            ProviderStreamEvent::Finish {
                output_tokens: 7,
                ..
            }
        ));
    }
}
