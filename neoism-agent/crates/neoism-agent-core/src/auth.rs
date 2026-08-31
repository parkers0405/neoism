use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum AuthInfo {
    Api {
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    OAuth {
        refresh: String,
        access: String,
        expires: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        enterprise_url: Option<String>,
    },
    WellKnown {
        key: String,
        token: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderAuthMethod {
    #[serde(rename = "type")]
    pub kind: ProviderAuthMethodKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<ProviderAuthPrompt>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderAuthMethodKind {
    Api,
    OAuth,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProviderAuthPrompt {
    Text {
        key: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        when: Option<PromptCondition>,
    },
    Select {
        key: String,
        message: String,
        options: Vec<SelectOption>,
        #[serde(skip_serializing_if = "Option::is_none")]
        when: Option<PromptCondition>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PromptCondition {
    pub key: String,
    pub op: PromptConditionOp,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptConditionOp {
    Eq,
    Neq,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthAuthorization {
    pub url: String,
    pub method: ProviderAuthAuthorizationMethod,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderAuthAuthorizationMethod {
    Auto,
    Code,
}

#[cfg(test)]
mod tests {
    use super::{ProviderAuthAuthorization, ProviderAuthAuthorizationMethod};

    #[test]
    fn authorization_uses_public_camel_case_field_names() {
        let value = serde_json::to_value(ProviderAuthAuthorization {
            url: "https://example.com/authorize".into(),
            method: ProviderAuthAuthorizationMethod::Auto,
            instructions: "Sign in".into(),
            attempt_id: Some("attempt_123".into()),
            expires_at: Some(123),
        })
        .unwrap();

        assert_eq!(value["attemptId"], "attempt_123");
        assert_eq!(value["expiresAt"], 123);
        assert!(value.get("attempt_id").is_none());
        assert!(value.get("expires_at").is_none());
    }
}
