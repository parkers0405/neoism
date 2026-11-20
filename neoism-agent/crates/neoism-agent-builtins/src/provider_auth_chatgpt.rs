//! "Sign in with ChatGPT": OpenAI's token-sharing sign-in for third-party agents, following
//! opencode's `plugin/provider/chatgpt.ts`.
//!
//! The first sign-in registers a client owned by the user (`dynamic_agent_client`); OpenAI
//! returns the issued client ID on the callback, and later sign-ins and refreshes reuse it.
//! The access token calls the platform API (`https://api.openai.com/v1`) against the user's
//! ChatGPT plan. Unlike the Codex flows this needs no fixed callback port and no device code.
use std::sync::Arc;

use neoism_agent_core::{
    AuthInfo, ProviderAuthAuthorization, ProviderAuthAuthorizationMethod,
};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::sync::{oneshot, Mutex, RwLock};

use super::provider_auth_util::{
    form_escape, neoism_user_agent, now_millis, pkce_challenge, random_oauth_string,
};
use crate::auth_store::AuthStore;
use crate::provider_auth_browser::{
    openai_browser_callback_outcome, parse_query, write_http_response,
};
use crate::ProviderOAuthPending;

pub(super) const METHOD_LABEL: &str = "Sign in with ChatGPT";
const ISSUER: &str = "https://auth.openai.com";
const REGISTRATION_CLIENT_ID: &str = "dynamic_agent_client";
const AGENT_NAME: &str = "Neoism";
const RESOURCE: &str = "https://api.openai.com/v1";
const TOKEN_SHARING_SCOPE: &str = "chatgpt.tokens.use.direct";
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// The authorization code and, on a first sign-in, the client ID OpenAI issued.
type ChatGptCallback = (String, Option<String>);

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

fn issuer() -> String {
    std::env::var("NEOISM_AGENT_OPENAI_OAUTH_ISSUER").unwrap_or_else(|_| ISSUER.to_string())
}

pub(super) async fn authorize_chatgpt(
    provider_id: &str,
    pending_key: &str,
    auth_store: &AuthStore,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<ProviderAuthAuthorization> {
    authorize_chatgpt_at(provider_id, pending_key, auth_store, pending, issuer(), host_id().await)
        .await
}

async fn authorize_chatgpt_at(
    provider_id: &str,
    pending_key: &str,
    auth_store: &AuthStore,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
    issuer: String,
    host_id: String,
) -> anyhow::Result<ProviderAuthAuthorization> {
    // Reauthorizing an existing connection must keep the client it registered.
    let client_id = match auth_store.get(provider_id).await {
        Ok(Some(AuthInfo::OAuth { client_id, .. })) => client_id,
        _ => None,
    };
    let code_verifier = random_oauth_string(64);
    let state = random_oauth_string(43);
    let nonce = random_oauth_string(43);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}{CALLBACK_PATH}",
        listener.local_addr()?.port()
    );
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(serve_callback(listener, state.clone(), sender));

    let mut params = vec![(
        "client_id",
        client_id
            .clone()
            .unwrap_or_else(|| REGISTRATION_CLIENT_ID.to_string()),
    )];
    if client_id.is_none() {
        params.push(("agent_name_hint", AGENT_NAME.to_string()));
    }
    params.extend([
        ("ext_agent_host_id", host_id),
        ("response_type", "code".to_string()),
        ("redirect_uri", redirect_uri.clone()),
        (
            "scope",
            format!("openid profile email offline_access resource.invoke {TOKEN_SHARING_SCOPE}"),
        ),
        ("resource", RESOURCE.to_string()),
        ("state", state),
        ("nonce", nonce.clone()),
        ("code_challenge_method", "S256".to_string()),
        ("code_challenge", pkce_challenge(&code_verifier)),
    ]);
    let url = format!(
        "{issuer}/api/accounts/authorize?{}",
        params
            .iter()
            .map(|(key, value)| format!("{key}={}", form_escape(value)))
            .collect::<Vec<_>>()
            .join("&")
    );
    // A new attempt replaces an abandoned one; dropping its receiver stops its listener.
    pending.write().await.insert(
        pending_key.to_string(),
        ProviderOAuthPending::OpenAiChatGpt {
            issuer,
            redirect_uri,
            code_verifier,
            nonce,
            client_id,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        },
    );
    Ok(ProviderAuthAuthorization {
        url,
        method: ProviderAuthAuthorizationMethod::Auto,
        instructions:
            "Complete authorization in your browser. This window will close automatically."
                .to_string(),
        attempt_id: None,
        expires_at: None,
    })
}

/// One listener per attempt on its own port: it answers until the matching callback (or a
/// cancel) arrives, the attempt is abandoned, or the wait runs out.
async fn serve_callback(
    listener: tokio::net::TcpListener,
    state: String,
    mut sender: oneshot::Sender<Result<ChatGptCallback, String>>,
) {
    let deadline = tokio::time::Instant::now() + CALLBACK_WAIT;
    loop {
        let accepted = tokio::select! {
            accepted = tokio::time::timeout_at(deadline, listener.accept()) => accepted,
            _ = sender.closed() => return,
        };
        let Ok(Ok((mut stream, _))) = accepted else {
            let _ = sender.send(Err("ChatGPT sign-in timed out".to_string()));
            return;
        };
        let mut buffer = vec![0_u8; 8192];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream.read(&mut buffer),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
        let request = String::from_utf8_lossy(&buffer[..read]);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        let outcome = openai_browser_callback_outcome(path, CALLBACK_PATH, &state);
        let _ = write_http_response(&mut stream, outcome.status, &outcome.body).await;
        if let Some(result) = outcome.result {
            let query = parse_query(path.split_once('?').map(|(_, query)| query).unwrap_or(""));
            let client_id = query.get("client_id").filter(|id| !id.is_empty()).cloned();
            let _ = sender.send(result.map(|code| (code, client_id)));
            return;
        }
    }
}

pub(super) async fn exchange_chatgpt(
    pending_key: &str,
    pending: &RwLock<std::collections::HashMap<String, ProviderOAuthPending>>,
) -> anyhow::Result<AuthInfo> {
    let (issuer, redirect_uri, code_verifier, nonce, saved_client_id, receiver) = {
        let pending = pending.read().await;
        let Some(ProviderOAuthPending::OpenAiChatGpt {
            issuer,
            redirect_uri,
            code_verifier,
            nonce,
            client_id,
            receiver,
        }) = pending.get(pending_key)
        else {
            anyhow::bail!("no pending ChatGPT sign-in for {pending_key}")
        };
        (
            issuer.clone(),
            redirect_uri.clone(),
            code_verifier.clone(),
            nonce.clone(),
            client_id.clone(),
            receiver.clone(),
        )
    };
    let receiver = receiver
        .lock()
        .await
        .take()
        .ok_or_else(|| anyhow::anyhow!("ChatGPT sign-in callback was already consumed"))?;
    let received = receiver.await;
    pending.write().await.remove(pending_key);
    let (code, returned_client_id) = received
        .map_err(|_| anyhow::anyhow!("ChatGPT sign-in callback listener stopped"))?
        .map_err(|error| anyhow::anyhow!(error))?;
    // Reauthorization callbacks may omit the client ID; reuse the one this attempt started with.
    if let (Some(saved), Some(returned)) = (&saved_client_id, &returned_client_id) {
        anyhow::ensure!(
            saved == returned,
            "ChatGPT returned a different client than this connection. Connect again."
        );
    }
    let client_id = saved_client_id.or(returned_client_id).ok_or_else(|| {
        anyhow::anyhow!("ChatGPT sign-in did not return a client ID. Connect again.")
    })?;
    let tokens = token_request(
        &reqwest::Client::new(),
        &issuer,
        std::time::Duration::from_secs(15),
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &code),
            ("code_verifier", &code_verifier),
            ("redirect_uri", &redirect_uri),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    anyhow::ensure!(
        tokens
            .scope
            .as_deref()
            .is_some_and(|scope| scope.split(' ').any(|item| item == TOKEN_SHARING_SCOPE)),
        "ChatGPT sign-in finished without token sharing. Sign in again and allow token sharing, or connect OpenAI with an API key."
    );
    let id_token = tokens
        .id_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("ChatGPT sign-in did not return an ID token."))?;
    verify_id_token(id_token, &issuer, &client_id, &nonce)?;
    credential(tokens, client_id, None)
}

/// Exchange a rotating refresh token. The caller serializes refreshes and persists the result.
pub(crate) async fn refresh_chatgpt(
    client: &reqwest::Client,
    issuer: &str,
    timeout: std::time::Duration,
    refresh_token: &str,
    client_id: &str,
) -> anyhow::Result<AuthInfo> {
    let tokens = token_request(
        client,
        issuer,
        timeout,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    credential(tokens, client_id.to_string(), Some(refresh_token))
}

async fn token_request(
    client: &reqwest::Client,
    issuer: &str,
    timeout: std::time::Duration,
    form: &[(&str, &str)],
) -> anyhow::Result<TokenResponse> {
    let response = client
        .post(format!("{issuer}/api/accounts/oauth/token"))
        .timeout(timeout)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("user-agent", neoism_user_agent())
        .body(
            form.iter()
                .map(|(key, value)| format!("{key}={}", form_escape(value)))
                .collect::<Vec<_>>()
                .join("&"),
        )
        .send()
        .await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        // Token-endpoint errors are short JSON without credentials; they explain refusals
        // such as an account that is not eligible for token sharing.
        let detail = String::from_utf8_lossy(&body[..body.len().min(400)]).to_string();
        anyhow::bail!("ChatGPT sign-in was refused ({status}): {detail}");
    }
    Ok(serde_json::from_slice(&body)?)
}

fn credential(
    tokens: TokenResponse,
    client_id: String,
    previous_refresh: Option<&str>,
) -> anyhow::Result<AuthInfo> {
    let refresh = tokens
        .refresh_token
        .filter(|token| !token.is_empty())
        .or_else(|| previous_refresh.map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!("ChatGPT sign-in did not return a refresh token."))?;
    Ok(AuthInfo::OAuth {
        refresh,
        access: tokens.access_token,
        expires: now_millis()
            .saturating_add(tokens.expires_in.unwrap_or(3_600).saturating_mul(1_000)),
        account_id: None,
        enterprise_url: None,
        client_id: Some(client_id),
    })
}

/// Check the ID token is for this client and this attempt. The token arrives straight from
/// the token endpoint over TLS, so its claims are checked without fetching the signing keys.
fn verify_id_token(
    token: &str,
    issuer: &str,
    client_id: &str,
    nonce: &str,
) -> anyhow::Result<()> {
    use base64::Engine;
    let invalid = || anyhow::anyhow!("ChatGPT sign-in returned an invalid ID token.");
    let payload = token.split('.').nth(1).ok_or_else(invalid)?;
    let claims: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    let audience_matches = match &claims["aud"] {
        serde_json::Value::String(audience) => audience == client_id,
        serde_json::Value::Array(audiences) => {
            audiences.iter().any(|audience| audience == client_id)
        }
        _ => false,
    };
    anyhow::ensure!(
        claims["iss"] == issuer
            && audience_matches
            && claims["nonce"] == nonce
            && claims["sub"].as_str().is_some_and(|sub| !sub.trim().is_empty())
            && claims["exp"]
                .as_u64()
                .is_some_and(|exp| exp.saturating_mul(1_000) > now_millis()),
        "ChatGPT sign-in returned an ID token for a different client or attempt."
    );
    Ok(())
}

/// A stable identifier for this install, sent so OpenAI can tell one host's sign-ins apart.
async fn host_id() -> String {
    let path = crate::default_cache_dir().join("chatgpt-agent-host-id");
    if let Ok(saved) = tokio::fs::read_to_string(&path).await {
        if saved.trim().starts_with("urn:uuid:") {
            return saved.trim().to_string();
        }
    }
    let mut bytes: [u8; 16] = rand::random();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let id = format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    );
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, &id).await;
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn id_token(claims: serde_json::Value) -> String {
        let encode = |value: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
        };
        format!("{}.{}.signature", encode(&serde_json::json!({"alg":"RS256"})), encode(&claims))
    }

    #[test]
    fn id_token_must_match_issuer_client_nonce_and_expiry() {
        let exp = now_millis() / 1_000 + 600;
        let claims = |edit: fn(&mut serde_json::Value)| {
            let mut claims = serde_json::json!({
                "iss": "https://issuer.test", "aud": "client", "nonce": "nonce", "sub": "user", "exp": exp,
            });
            edit(&mut claims);
            id_token(claims)
        };
        let check = |token: String| verify_id_token(&token, "https://issuer.test", "client", "nonce");
        assert!(check(claims(|_| {})).is_ok());
        assert!(check(claims(|c| c["aud"] = serde_json::json!(["other", "client"]))).is_ok());
        assert!(check(claims(|c| c["aud"] = "other".into())).is_err());
        assert!(check(claims(|c| c["nonce"] = "replayed".into())).is_err());
        assert!(check(claims(|c| c["iss"] = "https://elsewhere.test".into())).is_err());
        assert!(check(claims(|c| c["exp"] = 1.into())).is_err());
        assert!(check("not-a-jwt".into()).is_err());
    }

    /// A stand-in token endpoint that answers every request with `body`.
    async fn token_endpoint(body: String) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..1 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0_u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap();
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (issuer, task)
    }

    #[tokio::test]
    async fn sign_in_registers_a_client_and_stores_a_token_sharing_credential() {
        // The token endpoint answers after the attempt starts, so the ID token can be minted
        // for the nonce that attempt actually used.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let store_path = std::env::temp_dir()
            .join(format!("neoism-chatgpt-sign-in-{}.json", rand::random::<u64>()));
        let auth_store = AuthStore::from_service(Arc::new(
            neoism_agent_service_api::LocalProviderCredentialStore::new(store_path),
        ));
        let pending = RwLock::new(std::collections::HashMap::new());
        let authorization = authorize_chatgpt_at(
            "openai", "openai", &auth_store, &pending, issuer.clone(), "urn:uuid:test-host".into(),
        )
        .await
        .unwrap();
        let query = parse_query(authorization.url.split_once('?').unwrap().1);
        assert!(authorization.url.starts_with(&format!("{issuer}/api/accounts/authorize?")));
        assert_eq!(query["client_id"], REGISTRATION_CLIENT_ID);
        assert_eq!(query["agent_name_hint"], AGENT_NAME);
        assert_eq!(query["resource"], RESOURCE);
        assert!(query["scope"].split(' ').any(|scope| scope == TOKEN_SHARING_SCOPE));
        assert!(query["ext_agent_host_id"].starts_with("urn:uuid:"));
        assert!(query["redirect_uri"].starts_with("http://127.0.0.1:"));

        let token = id_token(serde_json::json!({
            "iss": issuer, "aud": "issued-client", "nonce": query["nonce"], "sub": "user",
            "exp": now_millis() / 1_000 + 600,
        }));
        let body = serde_json::json!({
            "access_token": "access", "refresh_token": "refresh", "id_token": token,
            "expires_in": 3600, "scope": format!("openid {TOKEN_SHARING_SCOPE}"),
        })
        .to_string();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 8192];
            let read = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });

        // The browser lands on the callback with the code and the issued client.
        let callback = format!(
            "{}?code=the-code&state={}&client_id=issued-client",
            query["redirect_uri"], query["state"]
        );
        let page = reqwest::get(&callback).await.unwrap();
        assert!(page.status().is_success());
        let auth = exchange_chatgpt("openai", &pending).await.unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /api/accounts/oauth/token"));
        for expected in ["grant_type=authorization_code", "client_id=issued-client", "code=the-code", "resource=https"] {
            assert!(request.contains(expected), "missing {expected} in {request}");
        }
        let AuthInfo::OAuth { access, refresh, client_id, account_id, .. } = auth else {
            panic!("expected an OAuth credential")
        };
        assert_eq!((access.as_str(), refresh.as_str()), ("access", "refresh"));
        assert_eq!(client_id.as_deref(), Some("issued-client"));
        assert!(account_id.is_none());
        assert!(pending.read().await.is_empty());
    }

    #[tokio::test]
    async fn refresh_keeps_the_client_and_the_old_refresh_token_when_none_is_rotated() {
        let (issuer, server) =
            token_endpoint(r#"{"access_token":"fresh","expires_in":60}"#.to_string()).await;
        let auth = refresh_chatgpt(
            &reqwest::Client::new(),
            &issuer,
            std::time::Duration::from_secs(5),
            "old-refresh",
            "issued-client",
        )
        .await
        .unwrap();
        let request = server.await.unwrap().remove(0);
        for expected in ["grant_type=refresh_token", "client_id=issued-client", "refresh_token=old-refresh"] {
            assert!(request.contains(expected), "missing {expected}");
        }
        assert!(matches!(auth, AuthInfo::OAuth { access, refresh, client_id, .. }
            if access == "fresh" && refresh == "old-refresh" && client_id.as_deref() == Some("issued-client")));
    }
}
