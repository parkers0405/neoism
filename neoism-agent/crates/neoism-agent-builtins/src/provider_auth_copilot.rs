use std::collections::BTreeMap;

use neoism_agent_core::{
    AuthInfo, ProviderAuthAuthorization, ProviderAuthAuthorizationMethod,
};
use serde::Deserialize;
use tokio::sync::RwLock;

use super::provider_auth_util::{neoism_user_agent, normalize_domain};
use crate::ProviderOAuthPending;

const COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
const COPILOT_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;

#[derive(Deserialize)]
struct CopilotDeviceResponse {
    verification_uri: String,
    user_code: String,
    device_code: String,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct CopilotTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

pub(super) async fn authorize_github_copilot(
    provider_id: &str,
    inputs: &BTreeMap<String, String>,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let enterprise_url = (inputs.get("deploymentType").map(String::as_str)
        == Some("enterprise"))
    .then(|| {
        inputs
            .get("enterpriseUrl")
            .map(|value| normalize_domain(value))
            .filter(|value| !value.is_empty())
    })
    .flatten();
    let domain = std::env::var("NEOISM_AGENT_COPILOT_OAUTH_DOMAIN")
        .ok()
        .or_else(|| enterprise_url.clone())
        .unwrap_or_else(|| "github.com".to_string());
    let device_code_url = format!("https://{domain}/login/device/code");
    let access_token_url = format!("https://{domain}/login/oauth/access_token");
    let client = reqwest::Client::new();
    let response = client
        .post(device_code_url)
        .header("accept", "application/json")
        .header("user-agent", neoism_user_agent())
        .json(&serde_json::json!({
            "client_id": COPILOT_CLIENT_ID,
            "scope": "read:user",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<CopilotDeviceResponse>()
        .await?;
    pending.write().await.insert(
        provider_id.to_string(),
        ProviderOAuthPending::GithubCopilot {
            access_token_url,
            device_code: response.device_code,
            interval_ms: response
                .interval
                .unwrap_or(5)
                .max(1)
                .saturating_mul(1_000)
                .saturating_add(COPILOT_POLLING_SAFETY_MARGIN_MS),
            enterprise_url,
        },
    );
    Ok(ProviderAuthAuthorization {
        url: response.verification_uri,
        method: ProviderAuthAuthorizationMethod::Auto,
        instructions: format!("Enter code: {}", response.user_code),
        attempt_id: None,
        expires_at: None,
    })
}

pub(super) async fn poll_github_copilot(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    let Some(ProviderOAuthPending::GithubCopilot {
        access_token_url,
        device_code,
        interval_ms,
        enterprise_url,
    }) = pending.write().await.remove(provider_id)
    else {
        anyhow::bail!("no pending GitHub Copilot OAuth flow for provider {provider_id}")
    };
    let client = reqwest::Client::new();
    let mut wait_ms = interval_ms;
    loop {
        let response = client
            .post(&access_token_url)
            .header("accept", "application/json")
            .header("user-agent", neoism_user_agent())
            .json(&serde_json::json!({
                "client_id": COPILOT_CLIENT_ID,
                "device_code": device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<CopilotTokenResponse>()
            .await?;
        if let Some(access) = response.access_token {
            return Ok(AuthInfo::OAuth {
                refresh: access.clone(),
                access,
                expires: 0,
                account_id: None,
                enterprise_url,
            });
        }
        match response.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                wait_ms = response
                    .interval
                    .unwrap_or_else(|| wait_ms / 1_000 + 5)
                    .saturating_mul(1_000)
                    .saturating_add(COPILOT_POLLING_SAFETY_MARGIN_MS);
            }
            Some(error) => anyhow::bail!("GitHub Copilot OAuth failed: {error}"),
            None => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
    }
}
