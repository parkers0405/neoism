use std::collections::HashMap;

use neoism_agent_core::ToolListItem;
use serde_json::Value;

pub(crate) fn provider_tool_map(tools: &[ToolListItem]) -> HashMap<String, ToolListItem> {
    tools
        .iter()
        .cloned()
        .map(|tool| (tool.id.clone(), tool))
        .collect()
}

pub(crate) fn normalize_provider_tool_name(
    name: &str,
    _input: &Value,
    available: &HashMap<String, ToolListItem>,
) -> Option<String> {
    available.contains_key(name).then(|| name.to_string())
}

pub(crate) fn tool_allowed_for_model(tool_id: &str, model_id: &str) -> bool {
    if use_apply_patch_for_model(model_id) {
        !matches!(tool_id, "edit" | "write")
    } else {
        tool_id != "apply_patch"
    }
}

pub(crate) fn use_apply_patch_for_model(model_id: &str) -> bool {
    let model_id = model_id.to_ascii_lowercase();
    is_openai_patch_model(&model_id)
}

fn is_openai_patch_model(model_id: &str) -> bool {
    if model_id.contains("oss") || model_id.contains("gpt-4") {
        return false;
    }
    if model_id == "stub" {
        return false;
    }
    model_id
        .split('/')
        .any(|part| part.starts_with("gpt-5") || part.contains("codex"))
}
