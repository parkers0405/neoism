use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use neoism_agent_core::{
    Id, IdKind, McpAuthStartResponse, McpConfig, McpOAuthConfig, McpOAuthSetting,
    McpStatus,
};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp_auth::{McpAuthClientInfo, McpAuthStore, McpAuthTokens};

#[path = "mcp_oauth_discovery.rs"]
mod mcp_oauth_discovery;

#[cfg(test)]
pub(super) use mcp_oauth_discovery::origin;
use mcp_oauth_discovery::{oauth_endpoints, OAuthEndpoints};

pub(super) async fn remote_auth_status_async(
    name: &str,
    url: &str,
    oauth: &Option<McpOAuthSetting>,
    auth_store: &McpAuthStore,
) -> McpStatus {
    let Some(oauth) = usable_oauth_config(oauth) else {
        return McpStatus::Connected;
    };
    match valid_tokens_for_url(name, url, auth_store) {
        Ok(Some(true)) => McpStatus::Connected,
        Ok(Some(false)) => match refresh_oauth_tokens(name, url, oauth, auth_store).await
        {
            Ok(true) => McpStatus::Connected,
            Ok(false) => McpStatus::NeedsAuth,
            Err(error) => McpStatus::Failed {
                error: error.to_string(),
            },
        },
        Ok(None) => McpStatus::NeedsAuth,
        Err(error) => McpStatus::Failed {
            error: error.to_string(),
        },
    }
}

pub(crate) async fn auth_start(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<McpAuthStartResponse> {
    let config = crate::config::load(directory)?.info.mcp;
    let remote = config
        .get(name)
        .ok_or_else(|| anyhow!("MCP server {name} is not configured"))?;
    let McpConfig::Remote { url, oauth, .. } = remote else {
        return Err(anyhow!("MCP server {name} does not support OAuth"));
    };
    let oauth = usable_oauth_config(oauth)
        .ok_or_else(|| anyhow!("MCP server {name} does not support OAuth"))?;
    let needs_registration = configured_client_id(oauth).is_none()
        && stored_client_info(name, url, auth_store)?.is_none();
    let endpoints = oauth_endpoints(url, oauth, true, false, needs_registration).await;
    let client =
        oauth_client_credentials(name, url, oauth, &endpoints, auth_store, true).await?;
    let authorization_url = endpoints.authorization_url;
    let redirect_uri = redirect_uri(name, oauth);
    let state = Id::ascending(IdKind::Entry).to_string();
    let code_verifier = random_oauth_string(64);
    let code_challenge = pkce_challenge(&code_verifier);
    let mut params = vec![
        ("response_type", "code".to_string()),
        ("client_id", client.client_id.clone()),
        ("redirect_uri", redirect_uri),
        ("state", state.clone()),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256".to_string()),
    ];
    if let Some(scope) = oauth.scope.as_ref().filter(|scope| !scope.is_empty()) {
        params.push(("scope", scope.clone()));
    }

    let mut entry = auth_store.get(name)?.unwrap_or_default();
    entry.tokens = None;
    entry.client_info = client.client_info;
    entry.code_verifier = Some(code_verifier);
    entry.oauth_state = Some(state.clone());
    entry.oauth_directory = Some(directory.to_string());
    entry.server_url = Some(url.clone());
    auth_store.set(name, entry)?;

    Ok(McpAuthStartResponse {
        authorization_url: append_query(&authorization_url, &params),
        oauth_state: state,
    })
}

pub(crate) async fn auth_callback(
    directory: &str,
    name: &str,
    code: &str,
    state: Option<&str>,
    auth_store: &McpAuthStore,
) -> anyhow::Result<McpStatus> {
    let config = crate::config::load(directory)?.info.mcp;
    let remote = config
        .get(name)
        .ok_or_else(|| anyhow!("MCP server {name} is not configured"))?;
    let McpConfig::Remote { url, oauth, .. } = remote else {
        return Err(anyhow!("MCP server {name} does not support OAuth"));
    };
    let oauth = usable_oauth_config(oauth)
        .ok_or_else(|| anyhow!("MCP server {name} does not support OAuth"))?;
    let entry = auth_store
        .get(name)?
        .ok_or_else(|| anyhow!("MCP OAuth flow for {name} was not started"))?;
    if let Some(state) = state {
        let expected = entry.oauth_state.as_deref().unwrap_or_default();
        if expected != state {
            return Err(anyhow!("MCP OAuth state mismatch"));
        }
    }
    let code_verifier = entry
        .code_verifier
        .as_deref()
        .ok_or_else(|| anyhow!("MCP OAuth code verifier is missing"))?;
    let endpoints = oauth_endpoints(url, oauth, false, true, false).await;
    let client =
        oauth_client_credentials(name, url, oauth, &endpoints, auth_store, false).await?;
    let token_url = endpoints.token_url;
    let redirect_uri = redirect_uri(name, oauth);
    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("client_id", client.client_id),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier.to_string()),
    ];
    if let Some(secret) = client.client_secret.filter(|secret| !secret.is_empty()) {
        params.push(("client_secret", secret));
    }

    let response = reqwest::Client::new()
        .post(&token_url)
        .header("accept", "application/json")
        .form(&params)
        .send()
        .await
        .with_context(|| format!("failed to exchange MCP OAuth code at {token_url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "MCP OAuth token exchange failed with {status}: {body}"
        ));
    }
    let tokens: OAuthTokenResponse = serde_json::from_str(&body)
        .context("failed to parse MCP OAuth token response")?;
    let mut entry = entry;
    entry.tokens = Some(McpAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: tokens.expires_at.or_else(|| {
            tokens
                .expires_in
                .map(|expires_in| unix_timestamp() + expires_in)
        }),
        scope: tokens.scope,
    });
    entry.code_verifier = None;
    entry.oauth_state = None;
    entry.oauth_directory = None;
    entry.server_url = Some(url.clone());
    auth_store.set(name, entry)?;

    Ok(super::status_for_entry(name, remote, auth_store))
}

pub(crate) fn authenticate_status(
    directory: &str,
    name: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<McpStatus> {
    let config = crate::config::load(directory)?.info.mcp;
    let remote = config
        .get(name)
        .ok_or_else(|| anyhow!("MCP server {name} is not configured"))?;
    Ok(super::status_for_entry(name, remote, auth_store))
}
pub(super) fn usable_oauth_config(
    oauth: &Option<McpOAuthSetting>,
) -> Option<&McpOAuthConfig> {
    match oauth {
        Some(McpOAuthSetting::Config(config)) => Some(config),
        Some(McpOAuthSetting::Disabled(_)) | None => None,
    }
}

fn configured_client_id(oauth: &McpOAuthConfig) -> Option<&str> {
    oauth.client_id.as_deref().filter(|value| !value.is_empty())
}

pub(super) fn valid_tokens_for_url(
    name: &str,
    url: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<Option<bool>> {
    let Some(entry) = auth_store.get_for_url(name, url)? else {
        return Ok(None);
    };
    let Some(tokens) = entry.tokens else {
        return Ok(None);
    };
    Ok(Some(!tokens_expired(&tokens)))
}

pub(super) async fn refresh_oauth_tokens(
    name: &str,
    url: &str,
    oauth: &McpOAuthConfig,
    auth_store: &McpAuthStore,
) -> anyhow::Result<bool> {
    let Some(entry) = auth_store.get_for_url(name, url)? else {
        return Ok(false);
    };
    let Some(existing_tokens) = entry.tokens else {
        return Ok(false);
    };
    let Some(refresh_token) = existing_tokens.refresh_token.clone() else {
        return Ok(false);
    };
    let endpoints = oauth_endpoints(url, oauth, false, true, false).await;
    let client =
        oauth_client_credentials(name, url, oauth, &endpoints, auth_store, false).await?;
    let mut params = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token),
        ("client_id", client.client_id),
    ];
    if let Some(secret) = client.client_secret.filter(|secret| !secret.is_empty()) {
        params.push(("client_secret", secret));
    }
    if let Some(scope) = oauth.scope.as_ref().filter(|scope| !scope.is_empty()) {
        params.push(("scope", scope.clone()));
    }

    let response = reqwest::Client::new()
        .post(&endpoints.token_url)
        .header("accept", "application/json")
        .form(&params)
        .send()
        .await
        .with_context(|| {
            format!(
                "failed to refresh MCP OAuth token at {}",
                endpoints.token_url
            )
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        if oauth_token_invalid_status(status) {
            let cleared = auth_store.clear_tokens(name, Some(url)).unwrap_or(false);
            tracing::warn!(
                mcp = name,
                url,
                status = %status,
                cleared,
                "MCP OAuth refresh was rejected; invalidated stored access token"
            );
            return Ok(false);
        }
        return Err(anyhow!(
            "MCP OAuth token refresh failed with {status}: {body}"
        ));
    }
    let tokens: OAuthTokenResponse = serde_json::from_str(&body)
        .context("failed to parse MCP OAuth refresh response")?;
    auth_store.update_tokens(
        name,
        McpAuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token.or(existing_tokens.refresh_token),
            expires_at: tokens.expires_at.or_else(|| {
                tokens
                    .expires_in
                    .map(|expires_in| unix_timestamp() + expires_in)
            }),
            scope: tokens.scope.or(existing_tokens.scope),
        },
        Some(url),
    )?;
    Ok(true)
}

fn oauth_token_invalid_status(status: StatusCode) -> bool {
    matches!(status, StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED)
}

fn tokens_expired(tokens: &McpAuthTokens) -> bool {
    tokens
        .expires_at
        .map(|expires_at| expires_at < unix_timestamp())
        .unwrap_or(false)
}

fn stored_client_info(
    name: &str,
    url: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<Option<McpAuthClientInfo>> {
    let Some(entry) = auth_store.get_for_url(name, url)? else {
        return Ok(None);
    };
    let Some(client_info) = entry.client_info else {
        return Ok(None);
    };
    if client_secret_expired(&client_info) {
        return Ok(None);
    }
    Ok(Some(client_info))
}

fn client_secret_expired(client_info: &McpAuthClientInfo) -> bool {
    client_info
        .client_secret_expires_at
        .map(|expires_at| expires_at != 0 && expires_at < unix_timestamp())
        .unwrap_or(false)
}

#[derive(Clone)]
struct OAuthClientCredentials {
    client_id: String,
    client_secret: Option<String>,
    client_info: Option<McpAuthClientInfo>,
}

async fn oauth_client_credentials(
    name: &str,
    url: &str,
    oauth: &McpOAuthConfig,
    endpoints: &OAuthEndpoints,
    auth_store: &McpAuthStore,
    allow_registration: bool,
) -> anyhow::Result<OAuthClientCredentials> {
    if let Some(client_id) = configured_client_id(oauth) {
        return Ok(OAuthClientCredentials {
            client_id: client_id.to_string(),
            client_secret: oauth
                .client_secret
                .as_ref()
                .filter(|secret| !secret.is_empty())
                .cloned(),
            client_info: None,
        });
    }

    if let Some(client_info) = stored_client_info(name, url, auth_store)? {
        return Ok(OAuthClientCredentials {
            client_id: client_info.client_id.clone(),
            client_secret: client_info.client_secret.clone(),
            client_info: Some(client_info),
        });
    }

    if allow_registration {
        if let Some(registration_url) = endpoints.registration_url.as_deref() {
            let client_info =
                register_oauth_client(name, url, oauth, registration_url, auth_store)
                    .await?;
            return Ok(OAuthClientCredentials {
                client_id: client_info.client_id.clone(),
                client_secret: client_info.client_secret.clone(),
                client_info: Some(client_info),
            });
        }
    }

    Err(anyhow!(
        "MCP OAuth client_id is required and no dynamic client registration endpoint was discovered"
    ))
}

async fn register_oauth_client(
    name: &str,
    url: &str,
    oauth: &McpOAuthConfig,
    registration_url: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<McpAuthClientInfo> {
    let redirect_uri = redirect_uri(name, oauth);
    let mut body = json!({
        "redirect_uris": [redirect_uri],
        "client_name": "neoism-agent",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if let Some(scope) = oauth.scope.as_ref().filter(|scope| !scope.is_empty()) {
        body["scope"] = Value::String(scope.clone());
    }

    let response = reqwest::Client::new()
        .post(registration_url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| {
            format!(
                "failed to dynamically register MCP OAuth client at {registration_url}"
            )
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "MCP OAuth client registration failed with {status}: {body}"
        ));
    }
    let registered: OAuthClientRegistrationResponse = serde_json::from_str(&body)
        .context("failed to parse MCP OAuth client registration response")?;
    let client_info = McpAuthClientInfo {
        client_id: registered.client_id,
        client_secret: registered.client_secret,
        client_id_issued_at: registered.client_id_issued_at,
        client_secret_expires_at: registered.client_secret_expires_at,
    };
    auth_store.update_client_info(name, client_info.clone(), Some(url))?;
    Ok(client_info)
}

pub(super) fn bearer_token_for_url(
    name: &str,
    url: &str,
    auth_store: &McpAuthStore,
) -> anyhow::Result<Option<String>> {
    Ok(auth_store
        .get_for_url(name, url)?
        .and_then(|entry| entry.tokens)
        .filter(|tokens| !tokens_expired(tokens))
        .map(|tokens| tokens.access_token))
}

fn redirect_uri(name: &str, oauth: &McpOAuthConfig) -> String {
    oauth
        .redirect_uri
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:4096/mcp/{name}/auth/callback"))
}

fn append_query(base: &str, params: &[(&str, String)]) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", form_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}{separator}{query}")
}

fn form_escape(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

fn random_oauth_string(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine;

    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Deserialize)]
struct OAuthClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    client_id_issued_at: Option<u64>,
    #[serde(default)]
    client_secret_expires_at: Option<u64>,
    #[allow(dead_code)]
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    token_type: Option<String>,
    #[allow(dead_code)]
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_for_url_ignores_expired_tokens() {
        let path = std::env::temp_dir().join(format!(
            "neoism-agent-mcp-oauth-{}.json",
            Id::ascending(IdKind::Event)
        ));
        let _ = std::fs::remove_file(&path);
        let store = McpAuthStore::new(path.clone());
        store
            .update_tokens(
                "remote",
                McpAuthTokens {
                    access_token: "old-token".to_string(),
                    refresh_token: Some("refresh-token".to_string()),
                    expires_at: Some(unix_timestamp().saturating_sub(1)),
                    scope: None,
                },
                Some("https://mcp.example.com"),
            )
            .unwrap();
        assert_eq!(
            bearer_token_for_url("remote", "https://mcp.example.com", &store).unwrap(),
            None
        );
        let _ = std::fs::remove_file(path);
    }
}
