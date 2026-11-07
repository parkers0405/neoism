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
use crate::ProviderOAuthPending;

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
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        authorize_openai_browser_inner(provider_id, pending, &issuer, &redirect_uri, OPENAI_BROWSER_OAUTH_PORT),
    )
    .await
    .map_err(|_| anyhow::anyhow!("OpenAI browser authorization preparation timed out"))?
}

async fn authorize_openai_browser_inner(
    provider_id: &str,
    pending_flows: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
    issuer: &str,
    redirect_uri: &str,
    port: u16,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let mut pending = pending_flows.write().await;
    if let Some(ProviderOAuthPending::OpenAiBrowser {
        issuer,
        redirect_uri,
        code_verifier,
        state,
        ..
    }) = pending.get(provider_id)
    {
        return Ok(openai_browser_authorization(
            issuer,
            redirect_uri,
            code_verifier,
            state,
        ));
    }
    let code_verifier = random_oauth_string(64);
    let state = random_oauth_string(43);
    let (sender, receiver) = oneshot::channel();
    if let Err(error) = start_openai_browser_callback_listener(
        port,
        "/auth/callback".to_string(),
        state.clone(),
        sender,
    )
    .await {
        if error.kind() != std::io::ErrorKind::AddrInUse {
            return Err(error.into());
        }
        // Another daemon (or Codex) owns the fixed redirect port. Recover
        // through device login without cancelling another app's login flow.
        drop(pending);
        return authorize_openai_headless_inner(provider_id, pending_flows, issuer).await;
    }
    pending.insert(
        provider_id.to_string(),
        ProviderOAuthPending::OpenAiBrowser {
            issuer: issuer.to_string(),
            redirect_uri: redirect_uri.to_string(),
            code_verifier: code_verifier.clone(),
            state: state.clone(),
            receiver: Arc::new(Mutex::new(Some(receiver))),
        },
    );

    Ok(openai_browser_authorization(
        issuer,
        redirect_uri,
        &code_verifier,
        &state,
    ))
}

fn openai_browser_authorization(
    issuer: &str,
    redirect_uri: &str,
    code_verifier: &str,
    state: &str,
) -> ProviderAuthAuthorization {
    let code_challenge = pkce_challenge(code_verifier);
    let params = [
        ("response_type", "code".to_string()),
        ("client_id", OPENAI_CLIENT_ID.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("scope", "openid profile email offline_access".to_string()),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256".to_string()),
        ("id_token_add_organizations", "true".to_string()),
        ("codex_cli_simplified_flow", "true".to_string()),
        ("state", state.to_string()),
        ("originator", "neoism".to_string()),
    ];
    ProviderAuthAuthorization {
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
        attempt_id: None,
        expires_at: None,
    }
}

pub(super) async fn exchange_openai_browser(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    tokio::time::timeout(
        std::time::Duration::from_secs(330),
        exchange_openai_browser_inner(provider_id, pending),
    )
    .await
    .map_err(|_| anyhow::anyhow!("OpenAI browser authorization timed out"))?
}

async fn exchange_openai_browser_inner(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    let (issuer, redirect_uri, code_verifier, receiver) = {
        let pending = pending.read().await;
        let Some(ProviderOAuthPending::OpenAiBrowser {
            issuer,
            redirect_uri,
            code_verifier,
            receiver,
            ..
        }) = pending.get(provider_id)
        else {
            anyhow::bail!(
                "no pending OpenAI OAuth browser flow for provider {provider_id}"
            )
        };
        (
            issuer.clone(),
            redirect_uri.clone(),
            code_verifier.clone(),
            receiver.clone(),
        )
    };
    let receiver = receiver.lock().await.take().ok_or_else(|| {
        anyhow::anyhow!("OpenAI OAuth browser callback was already consumed")
    })?;
    let code =
        match tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await {
            Ok(Ok(Ok(code))) => Ok(code),
            Ok(Ok(Err(error))) => Err(anyhow::anyhow!(error)),
            Ok(Err(_)) => Err(anyhow::anyhow!(
                "OpenAI OAuth browser callback listener stopped"
            )),
            Err(_) => Err(anyhow::anyhow!("OpenAI OAuth browser callback timed out")),
        };
    pending.write().await.remove(provider_id);
    let code = code?;
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
        .timeout(std::time::Duration::from_secs(15))
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
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        authorize_openai_headless_inner(provider_id, pending, &issuer),
    )
    .await
    .map_err(|_| anyhow::anyhow!("OpenAI device authorization preparation timed out"))?
}

async fn authorize_openai_headless_inner(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
    issuer: &str,
) -> anyhow::Result<ProviderAuthAuthorization> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{issuer}/api/accounts/deviceauth/usercode"))
        .header("user-agent", neoism_user_agent())
        .json(&serde_json::json!({ "client_id": OPENAI_CLIENT_ID }))
        .timeout(std::time::Duration::from_secs(15))
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
            issuer: issuer.to_string(),
            device_auth_id: response.device_auth_id,
            user_code: response.user_code.clone(),
            interval_ms,
        },
    );
    Ok(ProviderAuthAuthorization {
        url: format!("{issuer}/codex/device"),
        method: ProviderAuthAuthorizationMethod::Auto,
        instructions: format!("Enter code: {}", response.user_code),
        attempt_id: None,
        expires_at: None,
    })
}

pub(super) async fn poll_openai_headless(
    provider_id: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    tokio::time::timeout(
        std::time::Duration::from_secs(300),
        poll_openai_headless_inner(provider_id, pending),
    )
    .await
    .map_err(|_| anyhow::anyhow!("OpenAI device authorization timed out"))?
}

async fn poll_openai_headless_inner(
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
            .timeout(std::time::Duration::from_secs(15))
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
                .timeout(std::time::Duration::from_secs(15))
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn occupied_browser_port_automatically_completes_device_login() {
        // Stand in for another daemon/Codex without touching the user's port.
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer_url = format!("http://{}", issuer.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/api/accounts/deviceauth/usercode", r#"{"device_auth_id":"device","user_code":"ABC-123","interval":"1"}"#),
                ("/api/accounts/deviceauth/token", r#"{"authorization_code":"code","code_verifier":"verifier"}"#),
                ("/oauth/token", r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#),
            ] {
                let (mut socket, _) = issuer.accept().await.unwrap();
                let mut buffer = vec![0; 8192];
                let mut read = 0;
                loop {
                    read += socket.read(&mut buffer[read..]).await.unwrap();
                    if let Some(end) = buffer[..read].windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buffer[..end]);
                        let length = headers.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                        }).unwrap_or(0);
                        if read >= end + 4 + length {
                            assert!(headers.starts_with(&format!("POST {path} HTTP/1.1")));
                            break;
                        }
                    }
                    assert!(read < buffer.len(), "unexpected OAuth request");
                }
                socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let pending = RwLock::new(std::collections::HashMap::new());
        let authorization = authorize_openai_browser_inner(
            "attempt", &pending, &issuer_url, "http://localhost/auth/callback",
            occupied.local_addr().unwrap().port(),
        ).await.unwrap();
        assert_eq!(authorization.url, format!("{issuer_url}/codex/device"));
        assert_eq!(authorization.instructions, "Enter code: ABC-123");
        let provider = neoism_agent_core::ProviderInfo {
            id: "openai".into(), name: "OpenAI".into(),
            source: neoism_agent_core::ProviderSource::Builtin,
            env: vec![], key: None,
            options: Default::default(), models: Default::default(),
        };
        // Clients still submit the browser method they originally selected.
        let auth = super::super::callback(
            "openai", "attempt", &serde_json::json!(0), None, &[provider], &pending,
        ).await.unwrap();
        assert!(matches!(auth, AuthInfo::OAuth { access, refresh, .. }
            if access == "access" && refresh == "refresh"));
        assert!(pending.read().await.is_empty());
        // Recovery must leave the other process's listener untouched.
        assert!(occupied.local_addr().is_ok());
        server.await.unwrap();
    }
}
