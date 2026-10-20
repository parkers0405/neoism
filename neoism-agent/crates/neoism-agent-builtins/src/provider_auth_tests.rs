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
fn openai_methods_offer_chatgpt_oauth_before_api_key() {
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
    assert!(ok.body.contains("Authorization received"));
    assert!(!ok.body.contains("abc 123"));
    assert!(!ok.body.contains("state=expected"));
}

#[test]
fn openai_browser_callback_preserves_error_cancel_and_unrelated_outcomes() {
    for (path, status, message) in [
        ("/auth/callback?state=expected", "400 Bad Request", "Missing authorization code"),
        ("/auth/callback?code=&state=expected", "400 Bad Request", "Missing authorization code"),
        ("/auth/callback?code=secret", "400 Bad Request", "Invalid OAuth state"),
        ("/auth/callback?code=secret&state=wrong", "400 Bad Request", "Invalid OAuth state"),
        ("/auth/callback?error=access_denied", "200 OK", "access_denied"),
        ("/cancel", "200 OK", "Login cancelled"),
    ] {
        let outcome = openai_browser_callback_outcome(path, "/auth/callback", "expected");
        assert_eq!(outcome.status, status);
        assert_eq!(outcome.result.unwrap().unwrap_err(), message);
        assert!(outcome.body.contains(neoism_agent_service_api::browser_pages::WORDMARK));
        assert!(outcome.body.contains(neoism_agent_service_api::browser_pages::WORDMARK_SCRIPT));
        assert!(!outcome.body.contains("secret"));
    }
    let unrelated = openai_browser_callback_outcome("/favicon.ico", "/auth/callback", "expected");
    assert_eq!(unrelated.status, "404 Not Found");
    assert_eq!(unrelated.body, "Not found");
    assert!(unrelated.result.is_none());
}

#[test]
fn openai_browser_callback_escapes_provider_error_description() {
    let outcome = openai_browser_callback_outcome(
        "/auth/callback?error=access_denied&error_description=%3Cscript%3Ebad%3C%2Fscript%3E",
        "/auth/callback",
        "expected",
    );
    assert_eq!(outcome.status, "200 OK");
    assert_eq!(outcome.result.unwrap().unwrap_err(), "<script>bad</script>");
    assert!(outcome.body.contains("&lt;script&gt;bad&lt;/script&gt;"));
    assert!(!outcome.body.contains("<script>bad</script>"));
}
