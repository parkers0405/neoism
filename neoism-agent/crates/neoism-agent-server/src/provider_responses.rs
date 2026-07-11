use std::collections::{BTreeMap, BTreeSet};

use neoism_agent_core::{
    ProviderMessage, ProviderRole, ProviderStreamEvent, ToolListItem,
};
use serde::Deserialize;
use serde_json::{json, Value};

const TEXT_ID: &str = "text";
const REASONING_ID: &str = "reasoning";

#[derive(Clone, Debug, Default)]
pub(crate) struct ResponsesSseParser {
    event: Option<String>,
    data: Vec<String>,
    text_started: bool,
    reasoning_started: bool,
    reasoning_summary_parts: BTreeSet<String>,
    active_reasoning_item_id: Option<String>,
    tool_calls: BTreeMap<String, ResponsesToolCallState>,
    tool_call_emitted: bool,
}

#[derive(Clone, Debug, Default)]
struct ResponseUsage {
    total_tokens: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

#[derive(Clone, Debug, Default)]
struct ResponsesToolCallState {
    call_id: String,
    name: String,
    arguments: String,
}

pub(crate) fn responses_request_body(
    model: impl Into<String>,
    variant: Option<&str>,
    messages: &[ProviderMessage],
    tools: &[ToolListItem],
) -> Value {
    let model = model.into();
    let model_id = model.clone();
    let mut instructions = responses_instructions(messages);
    // Neoism's client-side equivalent of Codex's ultra mode: the backend
    // gets a plain deep reasoning effort (see responses_reasoning_options)
    // while subagent orchestration happens on our side — the model is told
    // to delegate proactively via the `task` tool. Codex does exactly this
    // (openai/codex core/src/context/multi_agent_mode_instructions.rs); its
    // ChatGPT backend rejects the platform API's `multi_agent` body param.
    if variant_is_ultra(variant) && tools.iter().any(|tool| tool.id == "task") {
        instructions.push_str(ULTRA_DELEGATION_INSTRUCTIONS);
    }
    let input = responses_input_items(messages);
    let mut body = json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "stream": true,
        "store": false,
    });
    let tools = responses_tools(&model_id, tools);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = Value::String("auto".to_string());
    }
    if let Some(reasoning) = responses_reasoning_options(&model_id, variant) {
        body["reasoning"] = reasoning;
    }
    body
}

const ULTRA_DELEGATION_INSTRUCTIONS: &str = "\n\nProactive multi-agent delegation is active. Use the task tool to spawn sub-agents whenever parallel work would materially improve speed or quality — fanning out across files, independent subtasks, or broad searches. Do not wait for the user to ask for delegation.";

fn variant_is_ultra(variant: Option<&str>) -> bool {
    variant.is_some_and(|value| value.trim().eq_ignore_ascii_case("ultra"))
}

/// GPT-5.6 (sol/terra/luna) introduced the `max` reasoning effort that
/// ultra rides on; older models reject it.
fn model_is_gpt_5_6(model_id: &str) -> bool {
    model_id.to_ascii_lowercase().contains("gpt-5.6")
}

#[allow(dead_code)]
pub(crate) fn parse_responses_sse_line(
    line: &str,
) -> anyhow::Result<Vec<ProviderStreamEvent>> {
    let mut parser = ResponsesSseParser::default();
    parser.push_line(line)
}

impl ResponsesSseParser {
    pub(crate) fn push_line(
        &mut self,
        line: &str,
    ) -> anyhow::Result<Vec<ProviderStreamEvent>> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.flush();
        }

        let Some((field, value)) = line.split_once(':') else {
            return Ok(Vec::new());
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => {
                self.event = Some(value.to_string());
                Ok(Vec::new())
            }
            "data" => {
                self.data.push(value.to_string());
                self.flush()
            }
            _ => Ok(Vec::new()),
        }
    }

    fn flush(&mut self) -> anyhow::Result<Vec<ProviderStreamEvent>> {
        if self.data.is_empty() {
            self.event = None;
            return Ok(Vec::new());
        }

        let data = self.data.join("\n");
        self.data.clear();
        if data.trim().is_empty() || data.trim() == "[DONE]" {
            self.event = None;
            return Ok(Vec::new());
        }

        let value: Value = serde_json::from_str(&data)?;
        let event = self.event.take().or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        let Some(event) = event else {
            return Ok(Vec::new());
        };

        let mut events = Vec::new();
        match event.as_str() {
            "response.output_text.delta" => {
                if let Some(delta) = string_field(&value, &["delta", "text"]) {
                    if !self.text_started {
                        events.push(ProviderStreamEvent::TextStart {
                            id: TEXT_ID.to_string(),
                        });
                        self.text_started = true;
                    }
                    events.push(ProviderStreamEvent::TextDelta {
                        id: TEXT_ID.to_string(),
                        delta,
                    });
                }
            }
            "response.reasoning_text.delta" => {
                if let Some(delta) = string_field(&value, &["delta", "text"]) {
                    if !self.reasoning_started {
                        events.push(ProviderStreamEvent::ReasoningStart {
                            id: REASONING_ID.to_string(),
                        });
                        self.reasoning_started = true;
                    }
                    events.push(ProviderStreamEvent::ReasoningDelta {
                        id: REASONING_ID.to_string(),
                        delta,
                    });
                }
            }
            "response.reasoning_summary_part.added" => {
                if let Some(event) = self.start_reasoning_summary_part(&value) {
                    events.push(event);
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(start) = self.start_reasoning_summary_part(&value) {
                    events.push(start);
                }
                if let (Some(id), Some(delta)) = (
                    reasoning_summary_part_id(
                        &value,
                        self.active_reasoning_item_id.as_deref(),
                    ),
                    string_field(&value, &["delta", "text"]),
                ) {
                    events.push(ProviderStreamEvent::ReasoningDelta { id, delta });
                }
            }
            "response.completed" | "response.incomplete" => {
                if let Some(output) = value
                    .get("response")
                    .and_then(|response| response.get("output"))
                    .and_then(Value::as_array)
                {
                    for item in output {
                        self.remember_tool_call(item);
                        if let Some(event) = self.finish_tool_call(item) {
                            events.push(event);
                        }
                        events.extend(self.finish_reasoning_item(item));
                    }
                }
                if self.text_started {
                    events.push(ProviderStreamEvent::TextEnd {
                        id: TEXT_ID.to_string(),
                    });
                    self.text_started = false;
                }
                if self.reasoning_started {
                    events.push(ProviderStreamEvent::ReasoningEnd {
                        id: REASONING_ID.to_string(),
                    });
                    self.reasoning_started = false;
                }
                events.extend(self.finish_all_reasoning_summary_parts());
                let usage = response_usage(&value);
                let finish = if self.tool_call_emitted {
                    self.tool_call_emitted = false;
                    Some("tool-calls".to_string())
                } else if event == "response.incomplete" {
                    Some("incomplete".to_string())
                } else {
                    string_field(&value, &["finish_reason", "status"])
                        .or_else(|| Some("stop".to_string()))
                };
                events.push(ProviderStreamEvent::FinishStep {
                    finish: finish.clone(),
                    total_tokens: usage.total_tokens,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    reasoning_tokens: usage.reasoning_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                });
                events.push(ProviderStreamEvent::Finish {
                    finish,
                    total_tokens: usage.total_tokens,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    reasoning_tokens: usage.reasoning_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                });
            }
            "response.failed" => {
                events.push(ProviderStreamEvent::Error {
                    message: error_message(&value),
                });
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item") {
                    if let Some(event) = self.start_reasoning_item(item) {
                        events.push(event);
                    }
                    self.remember_tool_call(item);
                }
            }
            "response.function_call_arguments.delta" => {
                self.append_tool_call_delta(&value);
            }
            "response.function_call_arguments.done" | "response.output_item.done" => {
                if let Some(item) = value.get("item") {
                    self.remember_tool_call(item);
                    if let Some(event) = self.finish_tool_call(item) {
                        events.push(event);
                    }
                    events.extend(self.finish_reasoning_item(item));
                } else if let Some(event) = self.finish_tool_call_from_value(&value) {
                    events.push(event);
                }
            }
            _ => {}
        }
        Ok(events)
    }

    fn remember_tool_call(&mut self, item: &Value) {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let Some(id) = tool_item_id(item) else {
            return;
        };
        let state = self.tool_calls.entry(id).or_default();
        if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
            state.call_id = call_id.to_string();
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            state.name = name.to_string();
        }
        if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
            state.arguments = arguments.to_string();
        }
    }

    fn start_reasoning_item(&mut self, item: &Value) -> Option<ProviderStreamEvent> {
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return None;
        }
        let item_id = tool_item_id(item)?;
        self.active_reasoning_item_id = Some(item_id.clone());
        self.start_reasoning_summary_id(format!("{item_id}:0"))
    }

    fn start_reasoning_summary_part(
        &mut self,
        value: &Value,
    ) -> Option<ProviderStreamEvent> {
        let id =
            reasoning_summary_part_id(value, self.active_reasoning_item_id.as_deref())?;
        self.start_reasoning_summary_id(id)
    }

    fn start_reasoning_summary_id(&mut self, id: String) -> Option<ProviderStreamEvent> {
        if !self.reasoning_summary_parts.insert(id.clone()) {
            return None;
        }
        Some(ProviderStreamEvent::ReasoningStart { id })
    }

    fn finish_reasoning_item(&mut self, item: &Value) -> Vec<ProviderStreamEvent> {
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return Vec::new();
        }
        let Some(item_id) = tool_item_id(item) else {
            return Vec::new();
        };
        let prefix = format!("{item_id}:");
        let finished = self
            .reasoning_summary_parts
            .iter()
            .filter(|part_id| part_id.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        for part_id in &finished {
            self.reasoning_summary_parts.remove(part_id);
        }
        if self.active_reasoning_item_id.as_deref() == Some(&item_id) {
            self.active_reasoning_item_id = None;
        }
        finished
            .into_iter()
            .map(|id| ProviderStreamEvent::ReasoningEnd { id })
            .collect()
    }

    fn finish_all_reasoning_summary_parts(&mut self) -> Vec<ProviderStreamEvent> {
        self.active_reasoning_item_id = None;
        let finished = std::mem::take(&mut self.reasoning_summary_parts);
        finished
            .into_iter()
            .map(|id| ProviderStreamEvent::ReasoningEnd { id })
            .collect()
    }

    fn append_tool_call_delta(&mut self, value: &Value) {
        let Some(id) = response_tool_call_id(value) else {
            return;
        };
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        self.tool_calls
            .entry(id)
            .or_default()
            .arguments
            .push_str(delta);
    }

    fn finish_tool_call_from_value(
        &mut self,
        value: &Value,
    ) -> Option<ProviderStreamEvent> {
        let id = response_tool_call_id(value)?;
        if let Some(arguments) = value.get("arguments").and_then(Value::as_str) {
            self.tool_calls.entry(id.clone()).or_default().arguments =
                arguments.to_string();
        }
        self.emit_tool_call(id)
    }

    fn finish_tool_call(&mut self, item: &Value) -> Option<ProviderStreamEvent> {
        let id = tool_item_id(item)?;
        self.emit_tool_call(id)
    }

    fn emit_tool_call(&mut self, id: String) -> Option<ProviderStreamEvent> {
        let state = self.tool_calls.remove(&id)?;
        if state.name.is_empty() {
            return None;
        }
        let input = parse_tool_arguments(&state.name, &state.arguments);
        self.tool_call_emitted = true;
        Some(ProviderStreamEvent::ToolCall {
            id: if state.call_id.is_empty() {
                id
            } else {
                state.call_id
            },
            name: state.name,
            input,
        })
    }
}

fn parse_tool_arguments(tool_name: &str, raw: &str) -> Value {
    match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_)
            if matches!(tool_name, "apply_patch" | "patch") && !raw.trim().is_empty() =>
        {
            json!({ "patchText": raw })
        }
        Err(_) => json!({}),
    }
}

fn responses_input_items(messages: &[ProviderMessage]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages
        .iter()
        .filter(|message| !matches!(message.role, ProviderRole::System))
    {
        match message.role {
            ProviderRole::User => input.push(responses_input_message(message)),
            ProviderRole::Assistant => {
                if !message.content.trim().is_empty() {
                    input.push(responses_input_message(message));
                }
                input.extend(message.tool_calls.iter().map(responses_function_call));
            }
            ProviderRole::Tool => {
                if let Some(call_id) = message.tool_call_id.as_deref() {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": message.content,
                    }));
                }
            }
            ProviderRole::System => {}
        }
    }
    input
}

fn responses_input_message(message: &ProviderMessage) -> Value {
    let role = match message.role {
        ProviderRole::System => "system",
        ProviderRole::User => "user",
        ProviderRole::Assistant => "assistant",
        ProviderRole::Tool => "tool",
    };
    let content_type = match message.role {
        ProviderRole::Assistant => "output_text",
        ProviderRole::System | ProviderRole::User | ProviderRole::Tool => "input_text",
    };

    let mut content = vec![json!({
        "type": content_type,
        "text": message.content,
    })];
    if matches!(message.role, ProviderRole::User) {
        content.extend(
            message
                .attachments
                .iter()
                .filter_map(responses_attachment_part),
        );
    }

    json!({
        "role": role,
        "content": content,
    })
}

fn responses_attachment_part(
    attachment: &neoism_agent_core::ProviderAttachment,
) -> Option<Value> {
    if !provider_media_url_supported(&attachment.url) {
        return None;
    }
    if attachment.mime.starts_with("image/") {
        return Some(json!({
            "type": "input_image",
            "image_url": attachment.url,
        }));
    }
    if attachment.mime == "application/pdf" {
        return Some(json!({
            "type": "input_file",
            "filename": attachment.filename.as_deref().unwrap_or("document.pdf"),
            "file_data": attachment.url,
        }));
    }
    None
}

fn provider_media_url_supported(url: &str) -> bool {
    url.starts_with("data:") || url.starts_with("https://") || url.starts_with("http://")
}

fn responses_function_call(call: &neoism_agent_core::ProviderToolCall) -> Value {
    let arguments =
        serde_json::to_string(&call.input).unwrap_or_else(|_| "{}".to_string());
    json!({
        "type": "function_call",
        "call_id": call.id,
        "name": call.name,
        "arguments": arguments,
        "status": "completed",
    })
}

fn responses_instructions(messages: &[ProviderMessage]) -> String {
    let instructions = messages
        .iter()
        .filter(|message| matches!(message.role, ProviderRole::System))
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if instructions.is_empty() {
        "You are Neoism, a concise coding assistant.".to_string()
    } else {
        instructions
    }
}

fn responses_tools(model_id: &str, tools: &[ToolListItem]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.id,
                "description": tool.description,
                "parameters": crate::provider_transform::tool_parameters(model_id, &tool.parameters),
            })
        })
        .collect()
}

fn responses_reasoning_options(model_id: &str, variant: Option<&str>) -> Option<Value> {
    let mut reasoning = serde_json::Map::new();
    // "ultra" maps to the deepest wire effort, exactly as Codex sends it
    // (ReasoningEffortConfig::Ultra => Max); the orchestration half of ultra
    // is client-side delegation instructions, not a request parameter.
    if variant_is_ultra(variant) && model_is_gpt_5_6(model_id) {
        reasoning.insert("effort".to_string(), Value::String("max".to_string()));
    } else if let Some(effort) = crate::provider::reasoning_effort(variant) {
        reasoning.insert("effort".to_string(), Value::String(effort.to_string()));
    } else if responses_model_uses_default_gpt5_reasoning(model_id) {
        reasoning.insert("effort".to_string(), Value::String("medium".to_string()));
    }
    if responses_model_supports_reasoning_summary(model_id) {
        reasoning.insert("summary".to_string(), Value::String("auto".to_string()));
    }
    if reasoning.is_empty() {
        None
    } else {
        Some(Value::Object(reasoning))
    }
}

fn responses_model_uses_default_gpt5_reasoning(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    is_gpt5_family(&lower)
        && !lower.contains("gpt-5-chat")
        && !lower.contains("gpt-5-pro")
}

fn responses_model_supports_reasoning_summary(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    responses_model_uses_default_gpt5_reasoning(&lower) || lower.contains("codex")
}

fn is_gpt5_family(model_id: &str) -> bool {
    model_id.split('/').any(|part| {
        part == "gpt-5"
            || part
                .strip_prefix("gpt-5")
                .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('.'))
    })
}

fn tool_item_id(item: &Value) -> Option<String> {
    item.get("id")
        .or_else(|| item.get("call_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn reasoning_summary_part_id(
    value: &Value,
    active_item_id: Option<&str>,
) -> Option<String> {
    let item_id = value
        .get("item_id")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .or(active_item_id)?;
    let summary_index = value
        .get("summary_index")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    Some(format!("{item_id}:{summary_index}"))
}

fn response_tool_call_id(value: &Value) -> Option<String> {
    value
        .get("item_id")
        .or_else(|| value.get("call_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(ToString::to_string)
}

fn response_usage(value: &Value) -> ResponseUsage {
    let usage = value.get("usage").or_else(|| {
        value
            .get("response")
            .and_then(|response| response.get("usage"))
    });
    let Some(usage) = usage else {
        return ResponseUsage::default();
    };

    ResponseUsage {
        total_tokens: usage
            .get("total_tokens")
            .or_else(|| usage.get("totalTokens"))
            .and_then(Value::as_u64),
        input_tokens: u64_field(usage, &["input_tokens", "prompt_tokens"]),
        output_tokens: u64_field(usage, &["output_tokens", "completion_tokens"]),
        reasoning_tokens: usage
            .get("output_tokens_details")
            .or_else(|| usage.get("completion_tokens_details"))
            .map(|details| u64_field(details, &["reasoning_tokens"]))
            .unwrap_or_default(),
        cache_read_tokens: usage
            .get("input_tokens_details")
            .or_else(|| usage.get("prompt_tokens_details"))
            .or_else(|| usage.get("inputTokenDetails"))
            .map(|details| {
                u64_field(
                    details,
                    &["cached_tokens", "cache_read_tokens", "cacheReadTokens"],
                )
            })
            .unwrap_or_default(),
        cache_write_tokens: usage
            .get("input_tokens_details")
            .or_else(|| usage.get("prompt_tokens_details"))
            .or_else(|| usage.get("inputTokenDetails"))
            .map(|details| {
                u64_field(details, &["cache_write_tokens", "cacheWriteTokens"])
            })
            .unwrap_or_default(),
    }
}

fn u64_field(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or_default()
}

fn error_message(value: &Value) -> String {
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        return message.to_string();
    }
    if let Some(message) = value
        .get("response")
        .and_then(|response| response.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        return message.to_string();
    }
    string_field(value, &["message"])
        .unwrap_or_else(|| "Responses API stream failed".to_string())
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ResponsesEventEnvelope {
    r#type: Option<String>,
}

#[cfg(test)]
#[path = "provider_responses_tests.rs"]
mod tests;
