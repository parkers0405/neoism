use neoism_agent_core::{
    ProviderGenerationRequest, ProviderGenerationResponse, ProviderMessage, ProviderRole,
    ProviderStreamEvent,
};
use serde_json::json;

use super::{estimate_tokens, ProviderEventStream, ProviderRuntime};

#[derive(Clone)]
pub(crate) struct StubRuntime;

impl ProviderRuntime for StubRuntime {
    fn stream(&self, request: ProviderGenerationRequest) -> ProviderEventStream {
        Box::pin(async_stream::try_stream! {
            if let Some(paths) = stub_parallel_read_tool_paths(&request) {
                let input_tokens = request
                    .messages
                    .iter()
                    .map(|message| estimate_tokens(&message.content))
                    .sum();
                yield ProviderStreamEvent::Start;
                yield ProviderStreamEvent::StartStep;
                for (index, path) in paths.iter().enumerate() {
                    let id = format!("call_parallel_read_{}", index + 1);
                    let input = json!({ "filePath": path });
                    yield ProviderStreamEvent::ToolInputStart {
                        id: id.clone(),
                        name: "read".to_string(),
                    };
                    yield ProviderStreamEvent::ToolInputDelta {
                        id: id.clone(),
                        delta: input.to_string(),
                    };
                    yield ProviderStreamEvent::ToolInputEnd { id: id.clone() };
                    yield ProviderStreamEvent::ToolCall {
                        id,
                        name: "read".to_string(),
                        input,
                    };
                }
                yield ProviderStreamEvent::FinishStep {
                    finish: Some("tool-calls".to_string()),
                    total_tokens: None,
                    input_tokens,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                };
                yield ProviderStreamEvent::Finish {
                    finish: Some("tool-calls".to_string()),
                    total_tokens: None,
                    input_tokens,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                };
                return;
            }
            if let Some(path) = stub_duplicate_patch_tool_path(&request) {
                let input = json!({
                    "patchText": format!(
                        "*** Begin Patch\n*** Update File: {path}\n@@\n+duplicate patch guard line\n*** End Patch"
                    )
                });
                let input_text = input.to_string();
                let input_tokens = request
                    .messages
                    .iter()
                    .map(|message| estimate_tokens(&message.content))
                    .sum();
                yield ProviderStreamEvent::Start;
                yield ProviderStreamEvent::StartStep;
                yield ProviderStreamEvent::ToolInputStart {
                    id: "call_patch_duplicate".to_string(),
                    name: "apply_patch".to_string(),
                };
                yield ProviderStreamEvent::ToolInputDelta {
                    id: "call_patch_duplicate".to_string(),
                    delta: input_text,
                };
                yield ProviderStreamEvent::ToolInputEnd {
                    id: "call_patch_duplicate".to_string(),
                };
                yield ProviderStreamEvent::ToolCall {
                    id: "call_patch_duplicate".to_string(),
                    name: "apply_patch".to_string(),
                    input: input.clone(),
                };
                yield ProviderStreamEvent::ToolCall {
                    id: "call_patch_duplicate".to_string(),
                    name: "apply_patch".to_string(),
                    input,
                };
                yield ProviderStreamEvent::FinishStep {
                    finish: Some("tool-calls".to_string()),
                    total_tokens: None,
                    input_tokens,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                };
                yield ProviderStreamEvent::Finish {
                    finish: Some("tool-calls".to_string()),
                    total_tokens: None,
                    input_tokens,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                };
                return;
            }
            if let Some(path) = stub_read_tool_path(&request) {
                let input = json!({ "filePath": path });
                let input_text = input.to_string();
                let input_tokens = request
                    .messages
                    .iter()
                    .map(|message| estimate_tokens(&message.content))
                    .sum();
                yield ProviderStreamEvent::Start;
                yield ProviderStreamEvent::StartStep;
                yield ProviderStreamEvent::ToolInputStart {
                    id: "call_read_1".to_string(),
                    name: "read".to_string(),
                };
                yield ProviderStreamEvent::ToolInputDelta {
                    id: "call_read_1".to_string(),
                    delta: input_text,
                };
                yield ProviderStreamEvent::ToolInputEnd {
                    id: "call_read_1".to_string(),
                };
                yield ProviderStreamEvent::ToolCall {
                    id: "call_read_1".to_string(),
                    name: "read".to_string(),
                    input,
                };
                yield ProviderStreamEvent::FinishStep {
                    finish: Some("tool-calls".to_string()),
                    total_tokens: None,
                    input_tokens,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                };
                yield ProviderStreamEvent::Finish {
                    finish: Some("tool-calls".to_string()),
                    total_tokens: None,
                    input_tokens,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                };
                return;
            }
            let response = stub_response(request);
            yield ProviderStreamEvent::Start;
            yield ProviderStreamEvent::StartStep;
            yield ProviderStreamEvent::TextStart { id: "text".to_string() };
            yield ProviderStreamEvent::TextDelta {
                id: "text".to_string(),
                delta: response.text.clone(),
            };
            yield ProviderStreamEvent::TextEnd { id: "text".to_string() };
            yield ProviderStreamEvent::FinishStep {
                finish: response.finish.clone(),
                total_tokens: response.total_tokens,
                input_tokens: response.input_tokens,
                output_tokens: response.output_tokens,
                reasoning_tokens: response.reasoning_tokens,
                cache_read_tokens: response.cache_read_tokens,
                cache_write_tokens: response.cache_write_tokens,
            };
            yield ProviderStreamEvent::Finish {
                finish: response.finish,
                total_tokens: response.total_tokens,
                input_tokens: response.input_tokens,
                output_tokens: response.output_tokens,
                reasoning_tokens: response.reasoning_tokens,
                cache_read_tokens: response.cache_read_tokens,
                cache_write_tokens: response.cache_write_tokens,
            };
        })
    }
}
fn stub_response(request: ProviderGenerationRequest) -> ProviderGenerationResponse {
    if let Some(output) = latest_tool_result(&request.messages) {
        let text = format!("Tool result received:\n{output}");
        return ProviderGenerationResponse {
            provider_id: "neoism".to_string(),
            model_id: "stub".to_string(),
            total_tokens: None,
            input_tokens: request
                .messages
                .iter()
                .map(|message| estimate_tokens(&message.content))
                .sum(),
            output_tokens: estimate_tokens(&text),
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            text,
            finish: Some("stop".to_string()),
        };
    }
    if let Some(error) = latest_tool_error(&request.messages) {
        let text = format!("Tool error received:\n{error}");
        return ProviderGenerationResponse {
            provider_id: "neoism".to_string(),
            model_id: "stub".to_string(),
            total_tokens: None,
            input_tokens: request
                .messages
                .iter()
                .map(|message| estimate_tokens(&message.content))
                .sum(),
            output_tokens: estimate_tokens(&text),
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            text,
            finish: Some("stop".to_string()),
        };
    }
    let prompt = request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, ProviderRole::User))
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .unwrap_or("your prompt");
    let text = format!(
        "neoism-agent Rust runtime is initialized. Provider streaming and tools are the next layer. Received: {prompt}"
    );
    ProviderGenerationResponse {
        provider_id: "neoism".to_string(),
        model_id: "stub".to_string(),
        total_tokens: None,
        input_tokens: request
            .messages
            .iter()
            .map(|message| estimate_tokens(&message.content))
            .sum(),
        output_tokens: estimate_tokens(&text),
        reasoning_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        text,
        finish: Some("stop".to_string()),
    }
}

fn stub_read_tool_path(request: &ProviderGenerationRequest) -> Option<String> {
    if let Some(paths) = stub_read_tool_chain_paths(request) {
        let index = read_tool_result_count(&request.messages);
        return paths.get(index).cloned();
    }
    if latest_tool_result(&request.messages).is_some()
        || latest_tool_error(&request.messages).is_some()
    {
        return None;
    }
    request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, ProviderRole::User))
        .and_then(|message| message.content.trim().strip_prefix("read-tool:"))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToString::to_string)
}

fn stub_parallel_read_tool_paths(
    request: &ProviderGenerationRequest,
) -> Option<Vec<String>> {
    if latest_tool_result(&request.messages).is_some()
        || latest_tool_error(&request.messages).is_some()
    {
        return None;
    }
    request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, ProviderRole::User))
        .and_then(|message| message.content.trim().strip_prefix("parallel-read-tools:"))
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|paths| paths.len() > 1)
}

fn stub_duplicate_patch_tool_path(request: &ProviderGenerationRequest) -> Option<String> {
    if latest_tool_result_for(&request.messages, "apply_patch").is_some()
        || latest_tool_error_for(&request.messages, "apply_patch").is_some()
    {
        return None;
    }
    request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, ProviderRole::User))
        .and_then(|message| message.content.trim().strip_prefix("duplicate-patch-tool:"))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToString::to_string)
}

fn latest_tool_result_for(
    messages: &[ProviderMessage],
    tool_name: &str,
) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        (matches!(message.role, ProviderRole::Tool)
            && message.tool_name.as_deref() == Some(tool_name)
            && message.tool_error != Some(true)
            && !message.content.trim().is_empty())
        .then(|| message.content.trim().to_string())
    })
}

fn latest_tool_error_for(
    messages: &[ProviderMessage],
    tool_name: &str,
) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        (matches!(message.role, ProviderRole::Tool)
            && message.tool_name.as_deref() == Some(tool_name)
            && message.tool_error == Some(true)
            && !message.content.trim().is_empty())
        .then(|| message.content.trim().to_string())
    })
}

fn stub_read_tool_chain_paths(
    request: &ProviderGenerationRequest,
) -> Option<Vec<String>> {
    request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, ProviderRole::User))
        .and_then(|message| message.content.trim().strip_prefix("read-tool-chain:"))
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|paths| !paths.is_empty())
}

fn read_tool_result_count(messages: &[ProviderMessage]) -> usize {
    let normalized = messages
        .iter()
        .filter(|message| {
            matches!(message.role, ProviderRole::Tool)
                && message.tool_name.as_deref() == Some("read")
                && message.tool_error != Some(true)
        })
        .count();
    let legacy = messages
        .iter()
        .map(|message| message.content.matches("[tool:read result]\n").count())
        .sum::<usize>();
    normalized + legacy
}

fn latest_tool_result(messages: &[ProviderMessage]) -> Option<String> {
    if let Some(output) = messages.iter().rev().find_map(|message| {
        (matches!(message.role, ProviderRole::Tool)
            && message.tool_name.as_deref() == Some("read")
            && message.tool_error != Some(true)
            && !message.content.trim().is_empty())
        .then(|| message.content.trim().to_string())
    }) {
        return Some(output);
    }
    messages.iter().rev().find_map(|message| {
        let marker = "[tool:read result]\n";
        message
            .content
            .find(marker)
            .map(|index| message.content[index + marker.len()..].trim().to_string())
            .filter(|output| !output.is_empty())
    })
}

fn latest_tool_error(messages: &[ProviderMessage]) -> Option<String> {
    if let Some(output) = messages.iter().rev().find_map(|message| {
        (matches!(message.role, ProviderRole::Tool)
            && message.tool_name.as_deref() == Some("read")
            && message.tool_error == Some(true)
            && !message.content.trim().is_empty())
        .then(|| message.content.trim().to_string())
    }) {
        return Some(output);
    }
    messages.iter().rev().find_map(|message| {
        let marker = "[tool:read error]\n";
        message
            .content
            .find(marker)
            .map(|index| message.content[index + marker.len()..].trim().to_string())
            .filter(|output| !output.is_empty())
    })
}
