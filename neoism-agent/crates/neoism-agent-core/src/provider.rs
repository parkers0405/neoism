use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{ProviderApiInfo, ToolListItem};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAttachment {
    pub mime: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReasoning {
    pub summary: Vec<Value>,
    pub encrypted_content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMessage {
    pub role: ProviderRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ProviderAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ProviderToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning: Vec<ProviderReasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_error: Option<bool>,
}

impl ProviderMessage {
    pub fn text(role: ProviderRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            reasoning: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            tool_error: None,
        }
    }

    pub fn assistant_tool_call(
        content: impl Into<String>,
        tool_calls: Vec<ProviderToolCall>,
    ) -> Self {
        Self {
            role: ProviderRole::Assistant,
            content: content.into(),
            attachments: Vec::new(),
            tool_calls,
            reasoning: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            tool_error: None,
        }
    }

    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        error: bool,
    ) -> Self {
        Self {
            role: ProviderRole::Tool,
            content: content.into(),
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            reasoning: Vec::new(),
            tool_call_id: Some(call_id.into()),
            tool_name: Some(name.into()),
            tool_error: error.then_some(true),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderGenerationRequest {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_verbosity: Option<crate::TextVerbosity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ProviderApiInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_env: Vec<String>,
    pub messages: Vec<ProviderMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolListItem>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderGenerationResponse {
    pub provider_id: String,
    pub model_id: String,
    pub text: String,
    pub finish: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProviderStreamEvent {
    Start,
    StartStep,
    TextStart {
        id: String,
    },
    TextDelta {
        id: String,
        delta: String,
    },
    TextEnd {
        id: String,
    },
    ReasoningStart {
        id: String,
    },
    ReasoningDelta {
        id: String,
        delta: String,
    },
    ReasoningEnd {
        id: String,
    },
    ReasoningMetadata {
        id: String,
        metadata: Value,
    },
    ToolInputStart {
        id: String,
        name: String,
    },
    ToolInputDelta {
        id: String,
        delta: String,
    },
    ToolInputEnd {
        id: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: String,
        output: String,
    },
    ToolError {
        id: String,
        message: String,
    },
    FinishStep {
        finish: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_tokens: Option<u64>,
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default)]
        reasoning_tokens: u64,
        #[serde(default)]
        cache_read_tokens: u64,
        #[serde(default)]
        cache_write_tokens: u64,
    },
    Finish {
        finish: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_tokens: Option<u64>,
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default)]
        reasoning_tokens: u64,
        #[serde(default)]
        cache_read_tokens: u64,
        #[serde(default)]
        cache_write_tokens: u64,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_event_uses_kebab_case_tags() {
        let event = ProviderStreamEvent::TextDelta {
            id: "txt".to_string(),
            delta: "hello".to_string(),
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "text-delta");
        assert_eq!(value["delta"], "hello");
    }
}
