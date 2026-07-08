use std::sync::Arc;

use neoism_agent_core::{
    AuthInfo, ProviderAuthAuthorization, ProviderAuthAuthorizationMethod,
};
use serde::Deserialize;
use tokio::sync::{oneshot, Mutex, RwLock};

use super::provider_auth_util::{
    extract_account_id_from_jwt, form_escape, neoism_user_agent, now_millis,
    pkce_challenge, random_oauth_string,
};
use crate::provider_auth_browser::start_openai_browser_callback_listener;
use crate::state::ProviderOAuthPending;

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_ISSUER: &str = "https://auth.openai.com";
const OPENAI_BROWSER_OAUTH_PORT: u16 = 1455;
const OPENAI_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;

#[derive(Deserialize)]
struct OpenAiDeviceResponse {
    device_auth_id: String,
    user_code: String,
    interval: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiDeviceTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Deserialize)]
struct OpenAiTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: Option<u64>,
    id_token: Option<String>,
}

pub(super) async fn authorize_openai_browser(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let issuer = std::env::var("NEOISM_AGENT_OPENAI_OAUTH_ISSUER")
        .unwrap_or_else(|_| OPENAI_ISSUER.to_string());
    let redirect_uri = std::env::var("NEOISM_AGENT_OPENAI_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| {
            format!("http://localhost:{OPENAI_BROWSER_OAUTH_PORT}/auth/callback")
        });
    let code_verifier = random_oauth_string(64);
    let code_challenge = pkce_challenge(&code_verifier);
    let state = random_oauth_string(43);
    let (sender, receiver) = oneshot::channel();
    start_openai_browser_callback_listener(
        OPENAI_BROWSER_OAUTH_PORT,
        "/auth/callback".to_string(),
        state.clone(),
        sender,
    )
    .await?;
    pending.write().await.insert(
        provider_id.to_string(),
        ProviderOAuthPending::OpenAiBrowser {
            issuer: issuer.clone(),
            redirect_uri: redirect_uri.clone(),
            code_verifier: code_verifier.clone(),
            state: state.clone(),
            receiver: Arc::new(Mutex::new(Some(receiver))),
        },
    );

    let params = [
        ("response_type", "code".to_string()),
        ("client_id", OPENAI_CLIENT_ID.to_string()),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email offline_access".to_string()),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256".to_string()),
        ("id_token_add_organizations", "true".to_string()),
        ("codex_cli_simplified_flow", "true".to_string()),
        ("state", state),
        ("originator", "neoism".to_string()),
    ];
    Ok(ProviderAuthAuthorization {
        url: format!(
            "{issuer}/oauth/authorize?{}",
            params
                .iter()
                .map(|(key, value)| format!("{key}={}", form_escape(value)))
                .collect::<Vec<_>>()
                .join("&")
        ),
        method: ProviderAuthAuthorizationMethod::Auto,
        instructions:
            "Complete authorization in your browser. This window will close automatically."
                .to_string(),
    })
}

pub(super) async fn exchange_openai_browser(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    let Some(ProviderOAuthPending::OpenAiBrowser {
        issuer,
        redirect_uri,
        code_verifier,
        state: _state,
        receiver,
    }) = pending.write().await.remove(provider_id)
    else {
        anyhow::bail!("no pending OpenAI OAuth browser flow for provider {provider_id}")
    };
    let receiver = receiver.lock().await.take().ok_or_else(|| {
        anyhow::anyhow!("OpenAI OAuth browser callback was already consumed")
    })?;
    let code = tokio::time::timeout(std::time::Duration::from_secs(300), receiver)
        .await
        .map_err(|_| anyhow::anyhow!("OpenAI OAuth browser callback timed out"))?
        .map_err(|_| anyhow::anyhow!("OpenAI OAuth browser callback listener stopped"))?
        .map_err(|error| anyhow::anyhow!(error))?;
    let token_response = reqwest::Client::new()
        .post(format!("{issuer}/oauth/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            form_escape(&code),
            form_escape(&redirect_uri),
            form_escape(OPENAI_CLIENT_ID),
            form_escape(&code_verifier),
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<OpenAiTokenResponse>()
        .await?;
    Ok(openai_auth_from_token_response(token_response))
}

pub(super) async fn authorize_openai_headless(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let issuer = std::env::var("NEOISM_AGENT_OPENAI_OAUTH_ISSUER")
        .unwrap_or_else(|_| OPENAI_ISSUER.to_string());
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{issuer}/api/accounts/deviceauth/usercode"))
        .header("user-agent", neoism_user_agent())
        .json(&serde_json::json!({ "client_id": OPENAI_CLIENT_ID }))
        .send()
        .await?
        .error_for_status()?
        .json::<OpenAiDeviceResponse>()
        .await?;
    let interval_ms = response
        .interval
        .as_deref()
        .and_then(|interval| interval.parse::<u64>().ok())
        .unwrap_or(5)
        .max(1)
        .saturating_mul(1_000)
        .saturating_add(OPENAI_POLLING_SAFETY_MARGIN_MS);
    pending.write().await.insert(
        provider_id.to_string(),
        ProviderOAuthPending::OpenAiHeadless {
            issuer: issuer.clone(),
            device_auth_id: response.device_auth_id,
            user_code: response.user_code.clone(),
            interval_ms,
        },
    );
    Ok(ProviderAuthAuthorization {
        url: format!("{issuer}/codex/device"),
        method: ProviderAuthAuthorizationMethod::Auto,
        instructions: format!("Enter code: {}", response.user_code),
    })
}

pub(super) async fn poll_openai_headless(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    let Some(ProviderOAuthPending::OpenAiHeadless {
        issuer,
        device_auth_id,
        user_code,
        interval_ms,
    }) = pending.write().await.remove(provider_id)
    else {
        anyhow::bail!("no pending OpenAI OAuth flow for provider {provider_id}")
    };
    let client = reqwest::Client::new();
    loop {
        let response = client
            .post(format!("{issuer}/api/accounts/deviceauth/token"))
            .header("user-agent", neoism_user_agent())
            .json(&serde_json::json!({
                "device_auth_id": device_auth_id,
                "user_code": user_code,
            }))
            .send()
            .await?;
        if response.status().is_success() {
            let device = response.json::<OpenAiDeviceTokenResponse>().await?;
            let token_response = client
                .post(format!("{issuer}/oauth/token"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(format!(
                    "grant_type=authorization_code&code={}&redirect_uri={}/deviceauth/callback&client_id={}&code_verifier={}",
                    form_escape(&device.authorization_code),
                    form_escape(&issuer),
                    form_escape(OPENAI_CLIENT_ID),
                    form_escape(&device.code_verifier),
                ))
                .send()
                .await?
                .error_for_status()?
                .json::<OpenAiTokenResponse>()
                .await?;
            return Ok(openai_auth_from_token_response(token_response));
        }
        if response.status().as_u16() != 403 && response.status().as_u16() != 404 {
            anyhow::bail!(
                "OpenAI OAuth polling failed with status {}",
                response.status()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
    }
}

fn openai_auth_from_token_response(token_response: OpenAiTokenResponse) -> AuthInfo {
    let account_id = token_response
        .id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&token_response.access_token));
    AuthInfo::OAuth {
        refresh: token_response.refresh_token,
        access: token_response.access_token,
        expires: now_millis().saturating_add(
            token_response
                .expires_in
                .unwrap_or(3_600)
                .saturating_mul(1_000),
        ),
        account_id,
        enterprise_url: None,
    }
}
