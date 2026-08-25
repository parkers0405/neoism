use super::provider_auth_util::pkce_challenge;
use super::*;
use neoism_agent_core::{PromptConditionOp, ProviderAuthPrompt, ProviderSource};

fn provider(id: &str, name: &str, env: Vec<&str>) -> ProviderInfo {
    ProviderInfo {
        id: id.to_string(),
        name: name.to_string(),
        source: ProviderSource::Builtin,
        env: env.into_iter().map(str::to_string).collect(),
        key: None,
        options: BTreeMap::new(),
        models: BTreeMap::new(),
    }
}

#[test]
fn openai_methods_match_opencode_chatgpt_oauth_order() {
    let methods = provider_methods(&provider("openai", "OpenAI", vec!["OPENAI_API_KEY"]));

    assert_eq!(
        methods
            .iter()
            .map(|method| method.label.as_str())
            .collect::<Vec<_>>(),
        vec![
            "ChatGPT Pro/Plus (browser)",
            "ChatGPT Pro/Plus (headless)",
            "Manually enter API Key"
        ]
    );
    assert!(matches!(methods[0].kind, ProviderAuthMethodKind::OAuth));
    assert!(matches!(methods[1].kind, ProviderAuthMethodKind::OAuth));
    assert!(matches!(methods[2].kind, ProviderAuthMethodKind::Api));
    assert!(methods[0].prompts.is_none());
    assert!(methods[1].prompts.is_none());
}

#[test]
fn github_copilot_oauth_method_includes_deployment_prompts() {
    let methods = provider_methods(&provider("github-copilot", "GitHub Copilot", vec![]));

    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].label, "GitHub Copilot");
    assert!(matches!(methods[0].kind, ProviderAuthMethodKind::OAuth));

    let prompts = methods[0].prompts.as_ref().expect("copilot prompts");
    assert_eq!(prompts.len(), 2);
    match &prompts[0] {
        ProviderAuthPrompt::Select {
            key,
            message,
            options,
            when,
        } => {
            assert_eq!(key, "deploymentType");
            assert_eq!(message, "Select GitHub deployment type");
            assert!(when.is_none());
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.value.as_str())
                    .collect::<Vec<_>>(),
                vec!["public", "enterprise"]
            );
        }
        prompt => panic!("expected deployment select prompt, got {prompt:?}"),
    }
    match &prompts[1] {
        ProviderAuthPrompt::Text {
            key,
            message,
            placeholder,
            when,
        } => {
            assert_eq!(key, "enterpriseUrl");
            assert_eq!(message, "Enter GitHub Enterprise URL");
            assert_eq!(placeholder.as_deref(), Some("https://github.example.com"));
            let when = when.as_ref().expect("enterpriseUrl condition");
            assert_eq!(when.key, "deploymentType");
            assert!(matches!(when.op, PromptConditionOp::Eq));
            assert_eq!(when.value, "enterprise");
        }
        prompt => panic!("expected enterprise URL text prompt, got {prompt:?}"),
    }
}

#[test]
fn generic_provider_offers_manual_oauth_token_method() {
    let methods = provider_methods(&provider("custom-oauth", "Custom OAuth", vec![]));

    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].label, "OAuth access token");
    assert!(matches!(methods[0].kind, ProviderAuthMethodKind::OAuth));
}

#[test]
fn pkce_challenge_matches_rfc_example() {
    assert_eq!(
        pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn openai_browser_callback_validates_state_and_extracts_code() {
    let ok = openai_browser_callback_outcome(
        "/auth/callback?code=abc%20123&state=expected",
        "/auth/callback",
        "expected",
    );
    assert_eq!(ok.status, "200 OK");
    assert_eq!(ok.result.unwrap().unwrap(), "abc 123");

    let bad = openai_browser_callback_outcome(
        "/auth/callback?code=abc&state=wrong",
        "/auth/callback",
        "expected",
    );
    assert_eq!(bad.status, "400 Bad Request");
    assert!(bad.result.unwrap().unwrap_err().contains("state"));
}
