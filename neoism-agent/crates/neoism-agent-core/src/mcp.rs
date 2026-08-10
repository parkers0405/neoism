use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpConfig {
    Local {
        #[serde(deserialize_with = "deserialize_command")]
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Vec<String>>,
        #[serde(default, alias = "env", skip_serializing_if = "Option::is_none")]
        environment: Option<BTreeMap<String, String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Remote {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<BTreeMap<String, String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        oauth: Option<McpOAuthSetting>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
}

fn deserialize_command<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(command) if !command.trim().is_empty() => Ok(vec![command]),
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(value) => Ok(value),
                other => Err(serde::de::Error::custom(format!(
                    "MCP command item must be a string, got {other}"
                ))),
            })
            .collect(),
        other => Err(serde::de::Error::custom(format!(
            "MCP command must be a string or string array, got {other}"
        ))),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum McpOAuthSetting {
    Disabled(bool),
    Config(McpOAuthConfig),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum McpStatus {
    Connected,
    Disabled,
    Failed { error: String },
    NeedsAuth,
    NeedsClientRegistration { error: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub name: String,
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub client: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
    pub client: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAuthStartResponse {
    pub authorization_url: String,
    pub oauth_state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAuthRemoveResponse {
    pub success: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResult {
    #[serde(default)]
    pub content: Vec<McpContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
    Resource {
        resource: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
    ResourceLink {
        uri: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NeoismConfig;
    use serde_json::json;

    #[test]
    fn config_decodes_typed_mcp_map() {
        let config: NeoismConfig = serde_json::from_value(json!({
            "mcp": {
                "local-example": {
                    "type": "local",
                    "command": ["node", "server.js"],
                    "environment": { "A": "B" },
                    "enabled": true,
                    "timeout": 1000
                },
                "local-opencode-style": {
                    "type": "local",
                    "command": "node",
                    "args": ["server.js"],
                    "env": { "A": "B" }
                },
                "remote-example": {
                    "type": "remote",
                    "url": "https://example.com/mcp",
                    "headers": { "Authorization": "Bearer token" },
                    "oauth": { "clientId": "client", "redirectUri": "http://127.0.0.1/callback" }
                }
            }
        }))
        .expect("config should decode");

        assert!(matches!(
            config.mcp["local-example"],
            McpConfig::Local { .. }
        ));
        match &config.mcp["local-opencode-style"] {
            McpConfig::Local {
                command,
                args,
                environment,
                ..
            } => {
                assert_eq!(command, &vec!["node".to_string()]);
                assert_eq!(args.as_deref(), Some(&["server.js".to_string()][..]));
                assert_eq!(
                    environment.as_ref().and_then(|env| env.get("A")),
                    Some(&"B".to_string())
                );
            }
            _ => panic!("expected local config"),
        }
        assert!(matches!(
            config.mcp["remote-example"],
            McpConfig::Remote { .. }
        ));
    }

    #[test]
    fn mcp_sdk_types_use_camel_case_fields() {
        let value = serde_json::to_value(McpToolCallResult {
            content: vec![McpContent::Image {
                data: "base64".to_string(),
                mime_type: "image/png".to_string(),
                annotations: None,
            }],
            is_error: Some(false),
        })
        .expect("result should serialize");

        assert_eq!(
            value,
            json!({
                "content": [{ "type": "image", "data": "base64", "mimeType": "image/png" }],
                "isError": false
            })
        );
    }
}
