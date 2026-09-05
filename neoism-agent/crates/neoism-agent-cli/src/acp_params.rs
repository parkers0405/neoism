use anyhow::Context;
use neoism_agent_core::{ModelRef, PromptPart, UserModel};
use serde_json::Value;

pub(super) fn cwd_from_params(params: &Value) -> Option<String> {
    string_param(params, "cwd")
        .or_else(|| string_param(params, "directory"))
        .or_else(|| string_param(params, "root"))
}

pub(super) fn required_string_param(params: &Value, key: &str) -> anyhow::Result<String> {
    string_param(params, key).with_context(|| format!("{key} is required"))
}

pub(super) fn string_param(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn model_param(params: &Value) -> Option<UserModel> {
    if let Some(value) = params.get("model").and_then(Value::as_str) {
        return parse_model_ref(value);
    }
    let model = params.get("model")?;
    let provider_id = model
        .get("providerID")
        .or_else(|| model.get("providerId"))
        .or_else(|| model.get("provider_id"))
        .and_then(Value::as_str)?;
    let model_id = model
        .get("modelID")
        .or_else(|| model.get("modelId"))
        .or_else(|| model.get("model_id"))
        .and_then(Value::as_str)?;
    Some(UserModel {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        connection_id: model
            .get("connectionId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        variant: model
            .get("variant")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

pub(super) fn parse_model_ref(value: &str) -> Option<UserModel> {
    let (provider_id, model_id) = value.split_once('/')?;
    Some(UserModel {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        connection_id: None,
        variant: None,
    })
}

pub(super) fn model_ref_from_user_model(model: UserModel) -> ModelRef {
    ModelRef {
        id: model.model_id,
        provider_id: model.provider_id,
        connection_id: model.connection_id,
        variant: model.variant,
    }
}

pub(crate) fn prompt_parts_from_acp(params: &Value) -> anyhow::Result<Vec<PromptPart>> {
    let Some(prompt) = params.get("prompt") else {
        if let Some(text) =
            string_param(params, "text").or_else(|| string_param(params, "message"))
        {
            return Ok(vec![PromptPart::Text { text }]);
        }
        anyhow::bail!("prompt is required");
    };
    let Some(items) = prompt.as_array() else {
        if let Some(text) = prompt.as_str() {
            return Ok(vec![PromptPart::Text {
                text: text.to_string(),
            }]);
        }
        anyhow::bail!("prompt must be a string or array");
    };
    let mut parts = Vec::new();
    for item in items {
        if let Some(text) = item.as_str() {
            parts.push(PromptPart::Text {
                text: text.to_string(),
            });
            continue;
        }
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    parts.push(PromptPart::Text {
                        text: text.to_string(),
                    });
                }
            }
            Some("resource") => {
                if let Some(text) = item
                    .get("resource")
                    .and_then(|resource| resource.get("text"))
                    .and_then(Value::as_str)
                {
                    parts.push(PromptPart::Text {
                        text: text.to_string(),
                    });
                }
            }
            Some("resource_link") => {
                if let Some(file) = prompt_file_part(item) {
                    parts.push(file);
                } else if let Some(uri) = item.get("uri").and_then(Value::as_str) {
                    parts.push(PromptPart::Text {
                        text: uri.to_string(),
                    });
                }
            }
            Some("file") | Some("image") => {
                if let Some(file) = prompt_file_part(item) {
                    parts.push(file);
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        anyhow::bail!("prompt did not contain any supported content");
    }
    Ok(parts)
}

fn prompt_file_part(item: &Value) -> Option<PromptPart> {
    let mime = item
        .get("mimeType")
        .or_else(|| item.get("mime_type"))
        .or_else(|| item.get("mime"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream")
        .to_string();
    let url = item
        .get("uri")
        .or_else(|| item.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            let data = item.get("data").and_then(Value::as_str)?;
            if data.starts_with("data:") {
                Some(data.to_string())
            } else {
                Some(format!("data:{mime};base64,{data}"))
            }
        })?;
    let filename = item
        .get("name")
        .or_else(|| item.get("filename"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| filename_from_url(&url));
    Some(PromptPart::File {
        url,
        filename,
        mime,
    })
}

fn filename_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.trim().is_empty() && !value.starts_with("data:"))
        .unwrap_or("attachment")
        .to_string()
}

pub(super) fn rpc_id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(super) fn permission_reply_from_acp_result(result: &Value) -> String {
    let selected = result
        .get("outcome")
        .and_then(|outcome| outcome.get("outcome"))
        .and_then(Value::as_str);
    if matches!(selected, Some(value) if value != "selected") {
        return "reject".to_string();
    }
    let reply = result
        .get("reply")
        .or_else(|| result.get("optionId"))
        .or_else(|| {
            result
                .get("outcome")
                .and_then(|outcome| outcome.get("optionId"))
        })
        .and_then(Value::as_str)
        .unwrap_or("reject");
    match reply {
        "always" => "always",
        "once" | "allow_once" => "once",
        _ => "reject",
    }
    .to_string()
}

pub(super) fn question_answers_from_acp_result(
    result: &Value,
) -> Option<Vec<Vec<String>>> {
    let value = result
        .get("answers")
        .or_else(|| {
            result
                .get("outcome")
                .and_then(|outcome| outcome.get("answers"))
        })
        .or_else(|| result.get("answer"))?;
    normalize_question_answers(value)
}

fn normalize_question_answers(value: &Value) -> Option<Vec<Vec<String>>> {
    if let Some(text) = value.as_str() {
        return Some(vec![vec![text.to_string()]]);
    }
    let array = value.as_array()?;
    if array.is_empty() {
        return Some(Vec::new());
    }
    let mut answers = Vec::new();
    for item in array {
        if let Some(text) = item.as_str() {
            answers.push(vec![text.to_string()]);
            continue;
        }
        let items = item
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        answers.push(items);
    }
    Some(answers)
}
