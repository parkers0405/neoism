use std::collections::BTreeMap;

use anyhow::Context;
use neoism_agent_core::{AuthInfo, ProviderStreamEvent};
use serde::Deserialize;

#[derive(Debug, Default)]
pub(super) struct ParsedStreamLine {
    pub(super) done: bool,
    pub(super) deltas: Vec<String>,
    pub(super) reasoning_deltas: Vec<String>,
    pub(super) tool_calls: Vec<OpenAiToolCallDelta>,
    pub(super) finish: Option<String>,
    pub(super) total_tokens: Option<u64>,
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) reasoning_tokens: Option<u64>,
    pub(super) cache_read_tokens: Option<u64>,
    pub(super) cache_write_tokens: Option<u64>,
}

pub(super) fn parse_stream_line(raw: &[u8]) -> anyhow::Result<ParsedStreamLine> {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let line = std::str::from_utf8(raw)?.trim();
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return Ok(ParsedStreamLine::default());
    };
    if data.is_empty() {
        return Ok(ParsedStreamLine::default());
    }
    if data == "[DONE]" {
        return Ok(ParsedStreamLine {
            done: true,
            ..ParsedStreamLine::default()
        });
    }

    let chunk: ChatCompletionChunk = serde_json::from_str(data)
        .context("failed to decode OpenAI-compatible streaming chunk")?;
    let mut parsed = ParsedStreamLine::default();
    if let Some(usage) = chunk.usage {
        if let Some(value) = usage.total_tokens {
            parsed.total_tokens = Some(value);
        }
        if let Some(value) = usage.prompt_tokens {
            parsed.input_tokens = Some(value);
        }
        if let Some(value) = usage.completion_tokens {
            parsed.output_tokens = Some(value);
        }
        if let Some(value) = usage
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens)
        {
            parsed.cache_read_tokens = Some(value);
        }
        if let Some(value) = usage
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens)
        {
            parsed.reasoning_tokens = Some(value);
        }
    }
    for choice in chunk.choices {
        if let Some(reason) = choice.finish_reason {
            parsed.finish = Some(reason);
        }
        let ChatCompletionDelta {
            content,
            reasoning,
            reasoning_content,
            tool_calls,
        } = choice.delta;
        if let Some(delta) = content.filter(|delta| !delta.is_empty()) {
            parsed.deltas.push(delta);
        }
        if let Some(delta) = reasoning_content
            .or(reasoning)
            .filter(|delta| !delta.is_empty())
        {
            parsed.reasoning_deltas.push(delta);
        }
        parsed
            .tool_calls
            .extend(tool_calls.into_iter().map(|tool_call| {
                let function = tool_call.function.unwrap_or_default();
                OpenAiToolCallDelta {
                    index: tool_call.index,
                    id: tool_call.id,
                    name: function.name,
                    arguments: function.arguments.unwrap_or_default(),
                }
            }));
    }
    Ok(parsed)
}

pub(super) fn handle_tool_call_deltas(
    tool_calls: &mut BTreeMap<usize, OpenAiToolCallState>,
    deltas: Vec<OpenAiToolCallDelta>,
) -> anyhow::Result<Vec<ProviderStreamEvent>> {
    let mut events = Vec::new();
    for delta in deltas {
        let state =
            tool_calls
                .entry(delta.index)
                .or_insert_with(|| OpenAiToolCallState {
                    id: delta
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("tool-{}", delta.index)),
                    name: delta.name.clone().unwrap_or_else(|| "unknown".to_string()),
                    arguments: String::new(),
                    started: false,
                    finished: false,
                });
        if state.finished {
            continue;
        }
        if let Some(id) = delta.id {
            state.id = id;
        }
        if let Some(name) = delta.name {
            state.name = name;
        }
        if !state.started {
            events.push(ProviderStreamEvent::ToolInputStart {
                id: state.id.clone(),
                name: state.name.clone(),
            });
            state.started = true;
        }
        if !delta.arguments.is_empty() {
            state.arguments.push_str(&delta.arguments);
            events.push(ProviderStreamEvent::ToolInputDelta {
                id: state.id.clone(),
                delta: delta.arguments,
            });
        }
        if let Ok(input) = serde_json::from_str(&state.arguments) {
            events.push(ProviderStreamEvent::ToolInputEnd {
                id: state.id.clone(),
            });
            events.push(ProviderStreamEvent::ToolCall {
                id: state.id.clone(),
                name: state.name.clone(),
                input,
            });
            state.finished = true;
        }
    }
    Ok(events)
}

pub(super) fn finish_open_tool_calls(
    tool_calls: &mut BTreeMap<usize, OpenAiToolCallState>,
) -> Vec<ProviderStreamEvent> {
    let mut events = Vec::new();
    for state in tool_calls.values_mut() {
        if state.finished || !state.started {
            continue;
        }
        events.push(ProviderStreamEvent::ToolInputEnd {
            id: state.id.clone(),
        });
        events.push(ProviderStreamEvent::ToolCall {
            id: state.id.clone(),
            name: state.name.clone(),
            input: parse_tool_input(&state.name, &state.arguments),
        });
        state.finished = true;
    }
    events
}

fn parse_tool_input(tool_name: &str, raw: &str) -> serde_json::Value {
    match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) if tool_name == "apply_patch" && !raw.trim().is_empty() => {
            serde_json::json!({ "patchText": raw })
        }
        Err(_) => serde_json::Value::String(raw.to_string()),
    }
}

pub(super) fn neoism_user_agent() -> String {
    format!("neoism-agent/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
pub(super) fn openai_key(auth: Option<&AuthInfo>) -> Option<String> {
    openai_key_with_fallback(auth, true)
}

pub(super) fn openai_key_with_fallback(
    auth: Option<&AuthInfo>,
    allow_openai_env_fallback: bool,
) -> Option<String> {
    match auth {
        Some(AuthInfo::Api { key, .. }) => Some(key.clone()),
        Some(AuthInfo::OAuth { access, .. }) => Some(access.clone()),
        _ if allow_openai_env_fallback => std::env::var("NEOISM_AGENT_OPENAI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .ok(),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    prompt_tokens_details: Option<ChatPromptTokenDetails>,
    completion_tokens_details: Option<ChatCompletionTokenDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatPromptTokenDetails {
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionTokenDetails {
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChatCompletionChunkChoice>,
    usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunkChoice {
    delta: ChatCompletionDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionDelta {
    content: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCallChunk>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallChunk {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiToolCallFunctionChunk>,
}

#[derive(Default, Debug, Deserialize)]
struct OpenAiToolCallFunctionChunk {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct OpenAiToolCallDelta {
    pub(super) index: usize,
    pub(super) id: Option<String>,
    pub(super) name: Option<String>,
    pub(super) arguments: String,
}

#[derive(Clone, Debug)]
pub(super) struct OpenAiToolCallState {
    id: String,
    name: String,
    arguments: String,
    started: bool,
    finished: bool,
}

pub(crate) fn estimate_tokens(text: &str) -> u64 {
    text.chars().count().saturating_add(3) as u64 / 4
}
