use neoism_agent_core::{
    ProviderAttachment, ProviderMessage, ProviderRole, ProviderToolCall, ToolListItem,
};
use serde_json::{json, Value};

pub(super) fn chat_completion_tools(
    model_id: &str,
    tools: &[ToolListItem],
) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.id,
                    "description": tool.description,
                    "parameters": crate::provider_transform::tool_parameters(model_id, &tool.parameters),
                }
            })
        })
        .collect()
}

pub(crate) fn reasoning_effort(value: Option<&str>) -> Option<&'static str> {
    match value
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("none") => Some("none"),
        Some("minimal") => Some("minimal"),
        Some("low") => Some("low"),
        Some("medium") => Some("medium"),
        Some("high") => Some("high"),
        Some("xhigh") | Some("max") => Some("xhigh"),
        // GPT-5.6's "ultra" is multi-agent orchestration, not an effort level;
        // surfaces without multi_agent support degrade it to the deepest effort.
        Some("ultra") => Some("xhigh"),
        _ => None,
    }
}

pub(super) fn chat_completion_messages(messages: &[ProviderMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message.role {
            ProviderRole::System => Some(json!({
                "role": "system",
                "content": message.content,
            })),
            ProviderRole::User => Some(json!({
                "role": "user",
                "content": chat_completion_content(message),
            })),
            ProviderRole::Assistant => {
                let mut item = json!({
                    "role": "assistant",
                    "content": if message.content.trim().is_empty() && !message.tool_calls.is_empty() {
                        Value::Null
                    } else {
                        Value::String(message.content.clone())
                    },
                });
                if !message.tool_calls.is_empty() {
                    item["tool_calls"] = Value::Array(
                        message
                            .tool_calls
                            .iter()
                            .map(chat_completion_tool_call)
                            .collect(),
                    );
                }
                Some(item)
            }
            ProviderRole::Tool => message.tool_call_id.as_ref().map(|tool_call_id| {
                json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": message.content,
                })
            }),
        })
        .collect()
}

fn chat_completion_content(message: &ProviderMessage) -> Value {
    let media = message
        .attachments
        .iter()
        .filter(|attachment| attachment.mime.starts_with("image/"))
        .filter(|attachment| provider_media_url_supported(&attachment.url))
        .collect::<Vec<_>>();
    if media.is_empty() {
        return Value::String(message.content.clone());
    }

    let mut content = Vec::new();
    if !message.content.trim().is_empty() {
        content.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }
    content.extend(media.into_iter().map(chat_completion_image_part));
    Value::Array(content)
}

fn chat_completion_image_part(attachment: &ProviderAttachment) -> Value {
    json!({
        "type": "image_url",
        "image_url": {
            "url": attachment.url,
        }
    })
}

fn provider_media_url_supported(url: &str) -> bool {
    url.starts_with("data:") || url.starts_with("https://") || url.starts_with("http://")
}

pub(super) fn chat_completion_tool_call(call: &ProviderToolCall) -> Value {
    let arguments =
        serde_json::to_string(&call.input).unwrap_or_else(|_| "{}".to_string());
    json!({
        "id": call.id,
        "type": "function",
        "function": {
            "name": call.name,
            "arguments": arguments,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_completion_user_messages_include_supported_images() {
        let mut message = ProviderMessage::text(ProviderRole::User, "Inspect this");
        message.attachments.push(ProviderAttachment {
            mime: "image/png".to_string(),
            url: "data:image/png;base64,abc".to_string(),
            filename: Some("shot.png".to_string()),
        });
        message.attachments.push(ProviderAttachment {
            mime: "application/pdf".to_string(),
            url: "data:application/pdf;base64,def".to_string(),
            filename: Some("report.pdf".to_string()),
        });

        let messages = chat_completion_messages(&[message]);

        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Inspect this");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,abc");
        assert_eq!(content.len(), 2);
    }
}
