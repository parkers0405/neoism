//! xAI Grok ("SuperGrok") OAuth — loopback + device-code flows, mirroring
//! opencode's `plugin/xai.ts`. Both yield an `AuthInfo::OAuth { access, refresh,
//! expires }`; the access token is sent as a Bearer against xAI's
//! OpenAI-compatible `https://api.x.ai/v1` endpoint, and refreshed on demand via
//! [`refresh_xai_oauth`]. We deliberately reuse the same public OAuth client id
//! as grok-cli/opencode because xAI only allowlists that client for the
//! `127.0.0.1:56121` loopback redirect.

use std::collections::HashMap;

use neoism_agent_core::{
    AuthInfo, ProviderAuthAuthorization, ProviderAuthAuthorizationMethod,
};
use serde::Deserialize;
use tokio::sync::RwLock;

use super::provider_auth_util::{
    form_escape, neoism_user_agent, now_millis, pkce_challenge, random_oauth_string,
};
use crate::state::ProviderOAuthPending;

const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_AUTHORIZE_URL: &str = "https://auth.x.ai/oauth2/authorize";
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const XAI_DEVICE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
// Must match the redirect the public client is registered with.
const XAI_LOOPBACK_PORT: u16 = 56121;
const XAI_CALLBACK_PATH: &str = "/callback";
const XAI_POLLING_SAFETY_MARGIN_MS: u64 = 1_000;

pub(super) fn is_headless_method(label: &str) -> bool {
    label.contains("Headless") || label.contains("Remote") || label.contains("VPS")
}

/// "SuperGrok Subscription" flow: return the browser authorization URL and keep
/// the PKCE verifier + redirect_uri. xAI redirects to
/// `127.0.0.1:56121/callback?code=…` after approval; the user copies that `code`
/// (from the page or the address bar) and pastes it → [`exchange_xai_loopback`].
/// We use `method: Code` so the GUI opens the browser AND shows a paste field.
pub(super) async fn authorize_xai_loopback(
    provider_id: &str,
    pending: &RwLock<HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let redirect_uri = format!("http://127.0.0.1:{XAI_LOOPBACK_PORT}{XAI_CALLBACK_PATH}");
    let code_verifier = random_oauth_string(64);
    let code_challenge = pkce_challenge(&code_verifier);
    let state = random_oauth_string(43);
    let nonce = random_oauth_string(43);
    pending.write().await.insert(
        provider_id.to_string(),
        ProviderOAuthPending::XaiLoopback {
            redirect_uri: redirect_uri.clone(),
            code_verifier,
        },
    );
    let params = [
        ("response_type", "code".to_string()),
        ("client_id", XAI_CLIENT_ID.to_string()),
        ("redirect_uri", redirect_uri),
        ("scope", XAI_SCOPE.to_string()),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256".to_string()),
        ("state", state),
        ("nonce", nonce),
        // xAI rejects the loopback client without these.
        ("plan", "generic".to_string()),
        ("referrer", "opencode".to_string()),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", form_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(ProviderAuthAuthorization {
        url: format!("{XAI_AUTHORIZE_URL}?{query}"),
        method: ProviderAuthAuthorizationMethod::Code,
        instructions: "Approve access in your browser. xAI then sends you to a \
            127.0.0.1 page — copy the `code` value it shows (or from the address \
            bar) and paste it here."
            .to_string(),
    })
}

pub(super) async fn exchange_xai_loopback(
    provider_id: &str,
    pending: &RwLock<HashMap<String, ProviderOAuthPending>>,
    code: &str,
) -> anyhow::Result<AuthInfo> {
    let Some(ProviderOAuthPending::XaiLoopback {
        redirect_uri,
        code_verifier,
    }) = pending.write().await.remove(provider_id)
    else {
        anyhow::bail!(
            "no pending xAI OAuth flow for provider {provider_id}; start /connect again"
        )
    };
    let code = extract_auth_code(code);
    let token = reqwest::Client::new()
        .post(XAI_TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("user-agent", neoism_user_agent())
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            form_escape(&code),
            form_escape(&redirect_uri),
            form_escape(XAI_CLIENT_ID),
            form_escape(&code_verifier),
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<XaiTokenResponse>()
        .await?;
    Ok(xai_auth_from_token(token))
}

/// Headless device-code flow: request a device/user code and return the
/// verification URL + user code as instructions. `callback` → [`poll_xai_device`].
pub(super) async fn authorize_xai_device(
    provider_id: &str,
    pending: &RwLock<HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let response = reqwest::Client::new()
        .post(XAI_DEVICE_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("user-agent", neoism_user_agent())
        .body(format!(
            "client_id={}&scope={}",
            form_escape(XAI_CLIENT_ID),
            form_escape(XAI_SCOPE),
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<XaiDeviceResponse>()
        .await?;
    let interval_ms = response
        .interval
        .unwrap_or(5)
        .max(1)
        .saturating_mul(1_000)
        .saturating_add(XAI_POLLING_SAFETY_MARGIN_MS);
    pending.write().await.insert(
        provider_id.to_string(),
        ProviderOAuthPending::XaiDevice {
            device_code: response.device_code,
            interval_ms,
        },
    );
    let url = response
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| response.verification_uri.clone());
    Ok(ProviderAuthAuthorization {
        url,
        method: ProviderAuthAuthorizationMethod::Auto,
        instructions: format!(
            "Go to {} and enter code: {}",
            response.verification_uri, response.user_code
        ),
    })
}

pub(super) async fn poll_xai_device(
    provider_id: &str,
    pending: &RwLock<HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    let Some(ProviderOAuthPending::XaiDevice {
        device_code,
        interval_ms,
    }) = pending.write().await.remove(provider_id)
    else {
        anyhow::bail!("no pending xAI OAuth device flow for provider {provider_id}")
    };
    let client = reqwest::Client::new();
    loop {
        let response = client
            .post(XAI_TOKEN_URL)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("user-agent", neoism_user_agent())
            .body(format!(
                "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={}&client_id={}",
                form_escape(&device_code),
                form_escape(XAI_CLIENT_ID),
            ))
            .send()
            .await?;
        if response.status().is_success() {
            let token = response.json::<XaiTokenResponse>().await?;
            return Ok(xai_auth_from_token(token));
        }
        // RFC 8628: keep polling on authorization_pending / slow_down; anything
        // else is a hard failure.
        let error = response
            .json::<XaiErrorResponse>()
            .await
            .ok()
            .and_then(|body| body.error)
            .unwrap_or_default();
        if error != "authorization_pending" && error != "slow_down" {
            anyhow::bail!("xAI OAuth device polling failed: {error}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
    }
}

/// Exchange a refresh token for a fresh access token (called by the runtime when
/// the stored access token is near expiry).
pub(crate) async fn refresh_xai_oauth(
    client: &reqwest::Client,
    refresh_token: &str,
) -> anyhow::Result<AuthInfo> {
    let token = client
        .post(XAI_TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("user-agent", neoism_user_agent())
        .body(format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}",
            form_escape(refresh_token),
            form_escape(XAI_CLIENT_ID),
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<XaiTokenResponse>()
        .await?;
    let mut auth = xai_auth_from_token(token);
    // xAI may not return a new refresh token; keep the existing one.
    if let AuthInfo::OAuth { refresh, .. } = &mut auth {
        if refresh.is_empty() {
            *refresh = refresh_token.to_string();
        }
    }
    Ok(auth)
}

/// Pull the authorization code out of whatever the user pasted: the bare code,
/// `code=…`, a `?code=…&state=…` fragment, or the full `127.0.0.1:56121/callback`
/// redirect URL.
fn extract_auth_code(pasted: &str) -> String {
    let pasted = pasted.trim();
    if let Some(index) = pasted.find("code=") {
        let after = &pasted[index + "code=".len()..];
        let end = after.find('&').unwrap_or(after.len());
        return after[..end].trim().to_string();
    }
    pasted.to_string()
}

fn xai_auth_from_token(token: XaiTokenResponse) -> AuthInfo {
    AuthInfo::OAuth {
        refresh: token.refresh_token.unwrap_or_default(),
        access: token.access_token,
        expires: now_millis()
            .saturating_add(token.expires_in.unwrap_or(3_600).saturating_mul(1_000)),
        account_id: None,
        enterprise_url: None,
    }
}

#[derive(Debug, Deserialize)]
struct XaiTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct XaiDeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct XaiErrorResponse {
    error: Option<String>,
}
