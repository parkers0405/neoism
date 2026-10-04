use std::time::Duration;

use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
use neoism_lua::{PluginNetworkRequest, PluginOwner};
use neoism_window::window::WindowId;

use crate::lua_async::{LuaAsyncSender, LuaAsyncToken};

pub(crate) fn spawn_request(
    owner: PluginOwner,
    id: String,
    window_id: WindowId,
    request: PluginNetworkRequest,
    token: LuaAsyncToken,
    sender: LuaAsyncSender,
    event_proxy: EventProxy,
    workspace: Option<String>,
) -> Result<(), String> {
    validate(&request)?;
    let authorization = match request.credential.as_deref() {
        Some(alias) => {
            let url = reqwest::Url::parse(&request.url)
                .map_err(|_| "network URL is invalid")?;
            Some(crate::credential_broker::broker().authorize_network(
                &owner,
                alias,
                workspace.as_deref(),
                url.host_str().unwrap_or_default(),
            )?)
        }
        None => None,
    };
    std::thread::Builder::new()
        .name("lua-network".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    sender.fail(owner, id, "runtime", error.to_string());
                    return;
                }
            };
            runtime.block_on(async move {
                let result = execute(&request, authorization.as_deref(), &token).await;
                if !token.is_cancelled() {
                    match result {
                        Ok(value) => sender.complete(owner, id, value),
                        Err((code, message)) => sender.fail(owner, id, code, message),
                    }
                    event_proxy
                        .send_event(RioEventType::Rio(RioEvent::Render), window_id);
                }
            });
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn validate(request: &PluginNetworkRequest) -> Result<(), String> {
    let url = reqwest::Url::parse(&request.url).map_err(|_| "network URL is invalid")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            "network requests require an HTTPS URL without embedded credentials".into(),
        );
    }
    if !matches!(
        request.method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
    ) {
        return Err("network method is not allowed".into());
    }
    if request
        .body
        .as_ref()
        .is_some_and(|body| body.len() > 1024 * 1024)
        || request.headers.len() > 64
    {
        return Err("network request payload exceeds limits".into());
    }
    for (name, value) in &request.headers {
        if name.len() > 128
            || value.len() > 16 * 1024
            || matches!(
                name.to_ascii_lowercase().as_str(),
                "authorization" | "proxy-authorization" | "cookie" | "host"
            )
        {
            return Err(format!("network header `{name}` is not allowed"));
        }
    }
    Ok(())
}

async fn execute(
    request: &PluginNetworkRequest,
    authorization: Option<&str>,
    token: &LuaAsyncToken,
) -> Result<serde_json::Value, (&'static str, String)> {
    let timeout = Duration::from_millis(
        request.timeout_millis.unwrap_or(30_000).clamp(100, 120_000),
    );
    let limit = request
        .max_response_bytes
        .unwrap_or(1024 * 1024)
        .clamp(1, 4 * 1024 * 1024);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| ("client", error.to_string()))?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|error| ("method", error.to_string()))?;
    let mut builder = client.request(method, &request.url);
    if let Some(value) = authorization {
        builder = builder.header(reqwest::header::AUTHORIZATION, value);
    }
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if let Some(body) = &request.body {
        builder = builder.body(body.clone());
    }
    let mut response = builder
        .send()
        .await
        .map_err(|error| ("request", error.to_string()))?;
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err((
            "response_too_large",
            "network response exceeds configured limit".into(),
        ));
    }
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "set-cookie" | "www-authenticate" | "proxy-authenticate"
            )
        })
        .take(64)
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ("response", error.to_string()))?
    {
        if token.is_cancelled() {
            return Err(("cancelled", "request cancelled".into()));
        }
        if body.len() + chunk.len() > limit {
            return Err((
                "response_too_large",
                "network response exceeds configured limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(
        serde_json::json!({ "status": status, "headers": headers, "body": String::from_utf8_lossy(&body) }),
    )
}

pub(crate) fn credential_available(
    owner: &PluginOwner,
    alias: &str,
    workspace: Option<&str>,
) -> Result<bool, String> {
    validate_credential_alias(alias)?;
    crate::credential_broker::broker().available(owner, alias, workspace)
}

fn validate_credential_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty()
        || alias.len() > 64
        || !alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("credential alias is invalid".into());
    }
    Ok(())
}
