use std::collections::BTreeMap;

use neoism_agent_core::{
    PromptCondition, PromptConditionOp, ProviderAuthMethod, ProviderAuthMethodKind,
    ProviderAuthPrompt, ProviderInfo, SelectOption,
};
use serde_json::Value;

pub(super) fn provider_methods(provider: &ProviderInfo) -> Vec<ProviderAuthMethod> {
    // Claude Code streams through the local Meridian proxy, so the common path
    // is a one-click "Connect Meridian" (no credential needed). Advanced users
    // who point the proxy at a real key can enter one instead.
    if provider.id == "claude-code" {
        return vec![
            ProviderAuthMethod {
                kind: ProviderAuthMethodKind::Api,
                label: "Connect Meridian".to_string(),
                prompts: None,
            },
            ProviderAuthMethod {
                kind: ProviderAuthMethodKind::Api,
                label: "Manually enter API Key".to_string(),
                prompts: Some(vec![ProviderAuthPrompt::Text {
                    key: "key".to_string(),
                    message: "Enter Claude Code API key".to_string(),
                    placeholder: Some("sk-ant-...".to_string()),
                    when: None,
                }]),
            },
        ];
    }
    if provider.id == "xai" {
        return vec![
            oauth_method("xAI Grok OAuth (SuperGrok Subscription)", None),
            oauth_method("xAI Grok OAuth (Headless / Remote / VPS)", None),
            ProviderAuthMethod {
                kind: ProviderAuthMethodKind::Api,
                label: "Manually enter API Key".to_string(),
                prompts: Some(api_prompts(provider)),
            },
        ];
    }
    let mut methods = Vec::new();
    if provider.id == "openai" {
        methods.push(oauth_method("ChatGPT Pro/Plus (browser)", None));
        methods.push(oauth_method("ChatGPT Pro/Plus (headless)", None));
    }
    if !provider.env.is_empty() || provider.id == "openai" {
        methods.push(ProviderAuthMethod {
            kind: ProviderAuthMethodKind::Api,
            label: if provider.id == "openai" {
                "Manually enter API Key".to_string()
            } else {
                "API key".to_string()
            },
            prompts: Some(api_prompts(provider)),
        });
    }
    if provider.id.starts_with("github-copilot") {
        methods.push(oauth_method(
            "GitHub Copilot",
            Some(github_copilot_prompts()),
        ));
    }
    if provider.id != "openai" && !provider.id.starts_with("github-copilot") {
        methods.push(oauth_method("OAuth access token", None));
    }
    methods
}

pub(super) fn oauth_method(
    label: &str,
    prompts: Option<Vec<ProviderAuthPrompt>>,
) -> ProviderAuthMethod {
    ProviderAuthMethod {
        kind: ProviderAuthMethodKind::OAuth,
        label: label.to_string(),
        prompts,
    }
}

pub(super) fn api_prompts(provider: &ProviderInfo) -> Vec<ProviderAuthPrompt> {
    let mut prompts = vec![ProviderAuthPrompt::Text {
        key: "key".to_string(),
        message: format!("Enter {} API key", provider.name),
        placeholder: provider.env.first().cloned(),
        when: None,
    }];

    if provider.id == "azure" && std::env::var("AZURE_RESOURCE_NAME").is_err() {
        prompts.push(ProviderAuthPrompt::Text {
            key: "resourceName".to_string(),
            message: "Enter Azure Resource Name".to_string(),
            placeholder: Some("my-models".to_string()),
            when: None,
        });
    }

    if provider.id == "cloudflare-workers-ai"
        && std::env::var("CLOUDFLARE_ACCOUNT_ID").is_err()
    {
        prompts.push(ProviderAuthPrompt::Text {
            key: "accountId".to_string(),
            message: "Enter your Cloudflare Account ID".to_string(),
            placeholder: Some("1234567890abcdef1234567890abcdef".to_string()),
            when: None,
        });
    }

    if provider.id == "cloudflare-ai-gateway" {
        if std::env::var("CLOUDFLARE_ACCOUNT_ID").is_err() {
            prompts.push(ProviderAuthPrompt::Text {
                key: "accountId".to_string(),
                message: "Enter your Cloudflare Account ID".to_string(),
                placeholder: Some("1234567890abcdef1234567890abcdef".to_string()),
                when: None,
            });
        }
        if std::env::var("CLOUDFLARE_GATEWAY_ID").is_err() {
            prompts.push(ProviderAuthPrompt::Text {
                key: "gatewayId".to_string(),
                message: "Enter your Cloudflare AI Gateway ID".to_string(),
                placeholder: Some("my-gateway".to_string()),
                when: None,
            });
        }
    }

    prompts
}

pub(super) fn github_copilot_prompts() -> Vec<ProviderAuthPrompt> {
    vec![
        ProviderAuthPrompt::Select {
            key: "deploymentType".to_string(),
            message: "Select GitHub deployment type".to_string(),
            options: vec![
                SelectOption {
                    label: "GitHub.com".to_string(),
                    value: "public".to_string(),
                    hint: None,
                },
                SelectOption {
                    label: "GitHub Enterprise".to_string(),
                    value: "enterprise".to_string(),
                    hint: Some("Requires enterprise URL".to_string()),
                },
            ],
            when: None,
        },
        ProviderAuthPrompt::Text {
            key: "enterpriseUrl".to_string(),
            message: "Enter GitHub Enterprise URL".to_string(),
            placeholder: Some("https://github.example.com".to_string()),
            when: Some(PromptCondition {
                key: "deploymentType".to_string(),
                op: PromptConditionOp::Eq,
                value: "enterprise".to_string(),
            }),
        },
    ]
}

pub(super) fn select_method(
    provider_id: &str,
    selector: &Value,
    providers: &[ProviderInfo],
) -> anyhow::Result<ProviderAuthMethod> {
    let provider = providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider {provider_id}"))?;
    let methods = provider_methods(provider);
    if methods.is_empty() {
        anyhow::bail!("provider {provider_id} does not expose auth methods")
    }

    if let Some(index) = method_index(selector) {
        return methods.get(index).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "auth method {index} does not exist for provider {provider_id}"
            )
        });
    }

    if let Some(kind) = selector.as_str() {
        return methods
            .into_iter()
            .find(|method| method_kind_name(&method.kind) == kind)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "auth method {kind} does not exist for provider {provider_id}"
                )
            });
    }

    anyhow::bail!("invalid auth method selector for provider {provider_id}")
}

pub(super) fn method_index(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .or_else(|| value.as_str()?.parse::<usize>().ok())
}

pub(super) fn method_kind_name(kind: &ProviderAuthMethodKind) -> &'static str {
    match kind {
        ProviderAuthMethodKind::Api => "api",
        ProviderAuthMethodKind::OAuth => "oauth",
    }
}

pub(super) fn provider_metadata(inputs: &BTreeMap<String, String>) -> Option<Value> {
    let metadata = inputs
        .iter()
        .filter(|(key, value)| key.as_str() != "key" && !value.trim().is_empty())
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect::<serde_json::Map<_, _>>();
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}
