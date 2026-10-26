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

pub(crate) fn tool_allowed_for_model(
    tool_id: &str,
    model_id: &str,
    batch_tool: Option<bool>,
) -> bool {
    if use_apply_patch_for_model(model_id, batch_tool) {
        !matches!(tool_id, "edit" | "write")
    } else {
        tool_id != "apply_patch"
    }
}

pub(crate) fn use_apply_patch_for_model(
    model_id: &str,
    batch_tool: Option<bool>,
) -> bool {
    let model_id = model_id.to_ascii_lowercase();
    batch_tool.unwrap_or(true) && is_openai_patch_model(&model_id)
}

fn is_openai_patch_model(model_id: &str) -> bool {
    if model_id.contains("oss") || model_id.contains("gpt-4") {
        return false;
    }
    if model_id == "stub" {
        return false;
    }
    model_id.split('/').any(|part| {
        part.starts_with("gpt-5") || part.starts_with("gpt-6") || part.contains("codex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_edit_defaults_on_for_codex_and_can_be_disabled() {
        for model in [
            "gpt-5.5",
            "gpt-6.1-sol",
            "openai/gpt-6-astra",
            "gpt-5.4-codex",
            "codex-mini-latest",
        ] {
            for setting in [None, Some(true)] {
                assert!(use_apply_patch_for_model(model, setting), "{model}");
                assert!(
                    tool_allowed_for_model("apply_patch", model, setting),
                    "{model}"
                );
                assert!(!tool_allowed_for_model("edit", model, setting), "{model}");
                assert!(!tool_allowed_for_model("write", model, setting), "{model}");
            }
            assert!(!use_apply_patch_for_model(model, Some(false)), "{model}");
            assert!(
                !tool_allowed_for_model("apply_patch", model, Some(false)),
                "{model}"
            );
            assert!(
                tool_allowed_for_model("edit", model, Some(false)),
                "{model}"
            );
            assert!(
                tool_allowed_for_model("write", model, Some(false)),
                "{model}"
            );
            assert!(
                tool_allowed_for_model("read", model, Some(false)),
                "{model}"
            );
        }
        assert!(!use_apply_patch_for_model("claude-sonnet-4", Some(true)));
    }

    #[test]
    fn gpt6_models_select_patch_tools_and_matching_prompt_guidance() {
        for model in [
            "gpt-6",
            "gpt-6.1-sol",
            "openai/gpt-6.1-sol",
            "openai/gpt-6-astra",
            "openrouter/openai/gpt-6.1-sol",
            "OPENAI/GPT-6.1-SOL",
        ] {
            assert!(use_apply_patch_for_model(model, None), "{model}");
            assert!(
                tool_allowed_for_model("apply_patch", model, None),
                "{model}"
            );
            assert!(!tool_allowed_for_model("edit", model, None), "{model}");
            assert!(!tool_allowed_for_model("write", model, None), "{model}");
            assert!(tool_allowed_for_model("read", model, None), "{model}");
        }
        for model in [
            "gpt-4.1",
            "gpt-oss-120b",
            "gpt-6-oss",
            "claude-sonnet-4",
            "stub",
        ] {
            assert!(!use_apply_patch_for_model(model, None), "{model}");
            assert!(
                !tool_allowed_for_model("apply_patch", model, None),
                "{model}"
            );
            assert!(tool_allowed_for_model("edit", model, None), "{model}");
            assert!(tool_allowed_for_model("write", model, None), "{model}");
        }
    }
}
