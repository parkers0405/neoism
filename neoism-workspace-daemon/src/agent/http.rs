use super::*;

pub(crate) async fn http_get_json(inner: &AgentInner, path: &str) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .http
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("agent-server GET {path}: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("agent-server GET {path}: HTTP {}", resp.status()));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server GET {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server GET {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_post_json(
    inner: &AgentInner,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .http
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|err| format!("agent-server POST {path}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!("agent-server POST {path}: HTTP {status}: {detail}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server POST {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server POST {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_patch_json(
    inner: &AgentInner,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .http
        .patch(&url)
        .json(body)
        .send()
        .await
        .map_err(|err| format!("agent-server PATCH {path}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!(
            "agent-server PATCH {path}: HTTP {status}: {detail}"
        ));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server PATCH {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server PATCH {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_put_json(
    inner: &AgentInner,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .http
        .put(&url)
        .json(body)
        .send()
        .await
        .map_err(|err| format!("agent-server PUT {path}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!("agent-server PUT {path}: HTTP {status}: {detail}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|err| format!("agent-server PUT {path}: body: {err}"))?;
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body)
        .map_err(|err| format!("agent-server PUT {path}: invalid JSON: {err}"))
}

pub(crate) async fn http_delete(inner: &AgentInner, path: &str) -> Result<(), String> {
    wait_for_local_agent_server(inner).await?;
    let url = format!("{}{}", inner.agent_server, path);
    let resp = inner
        .http
        .delete(&url)
        .send()
        .await
        .map_err(|err| format!("agent-server DELETE {path}: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "agent-server DELETE {path}: HTTP {}",
            resp.status()
        ));
    }
    Ok(())
}

pub(crate) async fn wait_for_local_agent_server(inner: &AgentInner) -> Result<(), String> {
    if local_bind_target(&inner.agent_server).is_none() {
        return Ok(());
    }
    let health_url = format!("{}{}", inner.agent_server, AGENT_SERVER_HEALTH_PATH);
    let started = tokio::time::Instant::now();
    loop {
        let current_error = match inner.http.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => format!("HTTP {}", resp.status()),
            Err(err) => err.to_string(),
        };
        if started.elapsed() >= AGENT_SERVER_READY_TIMEOUT {
            return Err(format!(
                "agent-server health {} did not become ready: {}",
                AGENT_SERVER_HEALTH_PATH, current_error
            ));
        }
        tokio::time::sleep(AGENT_SERVER_READY_POLL).await;
    }
}

pub(crate) async fn post_no_body(inner: &AgentInner, path: &str) -> Result<Value, String> {
    http_post_json(inner, path, &json!({})).await
}

pub(crate) fn split_model_ref(model: &str) -> Option<(String, String)> {
    let (provider, id) = model.split_once('/')?;
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider.to_string(), id.to_string()))
}

pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

