use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_stream::StreamExt;

const MAX_MCP_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) type NotificationHandler = Arc<dyn Fn(McpNotification) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct McpNotification {
    pub(crate) method: String,
    pub(crate) params: Value,
}

pub(crate) struct StdioJsonRpcClient {
    connection: Mutex<StdioConnection>,
    request_timeout: Duration,
    notifications: Option<NotificationHandler>,
}

struct StdioConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl StdioJsonRpcClient {
    pub(crate) async fn start(
        directory: &str,
        command: &[String],
        environment: Option<BTreeMap<String, String>>,
        request_timeout: Duration,
        notifications: Option<NotificationHandler>,
    ) -> anyhow::Result<Self> {
        let executable = command
            .first()
            .ok_or_else(|| anyhow!("MCP local server is missing a command"))?;
        let mut process = Command::new(executable);
        process
            .args(command.iter().skip(1))
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(environment) = environment {
            process.envs(environment);
        }
        crate::tool::process::set_new_process_group(&mut process);
        let mut child = process.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to open MCP stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to open MCP stdout"))?;
        Ok(Self {
            connection: Mutex::new(StdioConnection {
                child,
                stdin,
                stdout: BufReader::new(stdout).lines(),
                next_id: 1,
            }),
            request_timeout,
            notifications,
        })
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        let mut connection = self.connection.lock().await;
        let id = connection.next_id;
        connection.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        write_json_line(&mut connection.stdin, &request).await?;
        loop {
            let line = match timeout(self.request_timeout, connection.stdout.next_line())
                .await
            {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => {
                    return Err(anyhow!("MCP server exited before responding"))
                }
                Ok(Err(error)) => {
                    return Err(error).context("failed to read MCP response")
                }
                Err(_) => return Err(anyhow!("MCP request {method} timed out")),
            };
            if line.len() > MAX_MCP_RESPONSE_BYTES {
                return Err(anyhow!(
                    "MCP response exceeds {} byte limit",
                    MAX_MCP_RESPONSE_BYTES
                ));
            }
            let value: Value = serde_json::from_str(&line)
                .with_context(|| "failed to parse MCP JSON-RPC line")?;
            if handle_notification_value(&value, self.notifications.as_ref()) {
                continue;
            }
            let response: RpcResponse = serde_json::from_value(value)
                .with_context(|| "failed to parse MCP JSON-RPC line")?;
            if response.id != Some(id) {
                continue;
            }
            if let Some(error) = response.error {
                return Err(anyhow!(
                    "MCP request {method} failed with {}: {}",
                    error.code,
                    error.message
                ));
            }
            return response
                .result
                .ok_or_else(|| anyhow!("MCP request {method} returned no result"));
        }
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().await;
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        write_json_line(&mut connection.stdin, &request).await
    }

    pub(crate) async fn shutdown(&self) {
        let mut connection = self.connection.lock().await;
        let _ = connection.child.start_kill();
        let _ = timeout(Duration::from_secs(2), connection.child.wait()).await;
    }
}

pub(crate) struct HttpJsonRpcClient {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    next_id: Mutex<u64>,
    session_id: Mutex<Option<HeaderValue>>,
    notifications: Option<NotificationHandler>,
}

impl HttpJsonRpcClient {
    pub(crate) fn new(
        url: &str,
        configured_headers: Option<&BTreeMap<String, String>>,
        bearer_token: Option<String>,
        request_timeout: Duration,
        notifications: Option<NotificationHandler>,
    ) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .context("failed to build MCP HTTP client")?;
        let mut headers = HeaderMap::new();
        if let Some(configured_headers) = configured_headers {
            for (name, value) in configured_headers {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .with_context(|| format!("invalid MCP HTTP header name {name:?}"))?;
                let value = HeaderValue::from_str(value).with_context(|| {
                    format!("invalid MCP HTTP header value for {name}")
                })?;
                headers.insert(name, value);
            }
        }
        if let Some(token) = bearer_token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .context("invalid MCP OAuth access token for Authorization header")?,
            );
        }
        Ok(Self {
            client,
            url: url.to_string(),
            headers,
            next_id: Mutex::new(1),
            session_id: Mutex::new(None),
            notifications,
        })
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        let mut next_id = self.next_id.lock().await;
        let id = *next_id;
        *next_id += 1;
        drop(next_id);

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let response = self.post_rpc(method, &request).await?;
        let response = response.ok_or_else(|| {
            anyhow!("MCP HTTP request {method} returned no response body")
        })?;
        if response.id != Some(id) {
            return Err(anyhow!(
                "MCP HTTP request {method} returned a mismatched id"
            ));
        }
        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP HTTP request {method} failed with {}: {}",
                error.code,
                error.message
            ));
        }
        response
            .result
            .ok_or_else(|| anyhow!("MCP HTTP request {method} returned no result"))
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let _ = self.post_rpc(method, &request).await?;
        Ok(())
    }

    async fn post_rpc(
        &self,
        method: &str,
        request: &Value,
    ) -> anyhow::Result<Option<RpcResponse>> {
        for attempt in 0..2 {
            let mut headers = self.headers.clone();
            if !headers.contains_key(CONTENT_TYPE) {
                headers
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            }
            if !headers.contains_key(ACCEPT) {
                headers.insert(
                    ACCEPT,
                    HeaderValue::from_static("application/json, text/event-stream"),
                );
            }
            let session_header = self.session_id.lock().await.clone();
            if let Some(session_id) = session_header.clone() {
                headers.insert(HeaderName::from_static("mcp-session-id"), session_id);
            }
            let response = self
                .client
                .post(&self.url)
                .headers(headers)
                .json(request)
                .send()
                .await
                .with_context(|| format!("MCP HTTP request {method} failed to send"))?;
            let status = response.status();
            let response_headers = response.headers().clone();
            if let Some(session_id) = response_headers
                .get(HeaderName::from_static("mcp-session-id"))
                .cloned()
            {
                *self.session_id.lock().await = Some(session_id);
            }
            let content_type = response_headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            if response
                .content_length()
                .is_some_and(|length| length > MAX_MCP_RESPONSE_BYTES as u64)
            {
                return Err(anyhow!(
                    "MCP HTTP response exceeds {} byte limit",
                    MAX_MCP_RESPONSE_BYTES
                ));
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.with_context(|| {
                    format!("failed to read MCP HTTP response for {method}")
                })?;
                if bytes.len().saturating_add(chunk.len()) > MAX_MCP_RESPONSE_BYTES {
                    return Err(anyhow!(
                        "MCP HTTP response exceeds {} byte limit",
                        MAX_MCP_RESPONSE_BYTES
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body = String::from_utf8(bytes).with_context(|| {
                format!("MCP HTTP response for {method} is not UTF-8")
            })?;
            if status == StatusCode::NOT_FOUND && session_header.is_some() && attempt == 0
            {
                *self.session_id.lock().await = None;
                tracing::debug!(
                    method,
                    url = %self.url,
                    "MCP HTTP session was rejected; clearing session id and retrying once"
                );
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!(
                    "MCP HTTP request {method} failed with {status}: {body}"
                ));
            }
            let events = parse_http_rpc_events(method, content_type.as_deref(), &body)?;
            for notification in events.notifications {
                if let Some(handler) = &self.notifications {
                    handler(notification);
                }
            }
            return Ok(events.response);
        }
        unreachable!("MCP HTTP request retry loop always returns")
    }

    pub(crate) fn spawn_sse_listener(self: &Arc<Self>) {
        let client = self.clone();
        tokio::spawn(async move {
            client.run_sse_listener().await;
        });
    }

    async fn run_sse_listener(self: Arc<Self>) {
        if self.notifications.is_none() {
            return;
        }
        let mut delay = Duration::from_millis(250);
        let mut saw_error = false;
        for _ in 0..5 {
            match self.connect_sse_once().await {
                Ok(true) => {
                    delay = Duration::from_millis(250);
                }
                Ok(false) => {
                    tracing::debug!(url = %self.url, "MCP HTTP server does not expose an SSE listener endpoint");
                    return;
                }
                Err(error) => {
                    saw_error = true;
                    tracing::debug!(
                        url = %self.url,
                        error = %error,
                        retry_in_ms = delay.as_millis(),
                        "MCP SSE listener disconnected; retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
            }
        }
        if saw_error {
            tracing::warn!(
                url = %self.url,
                "MCP SSE listener stopped after repeated reconnect failures"
            );
        } else {
            tracing::debug!(
                url = %self.url,
                "MCP SSE listener stopped after repeated clean disconnects"
            );
        }
    }

    async fn connect_sse_once(&self) -> anyhow::Result<bool> {
        let mut headers = self.headers.clone();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if let Some(session_id) = self.session_id.lock().await.clone() {
            headers.insert(HeaderName::from_static("mcp-session-id"), session_id);
        }
        let response = self.client.get(&self.url).headers(headers).send().await?;
        let status = response.status();
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Ok(false);
        }
        if !status.is_success() {
            tracing::debug!(
                url = %self.url,
                %status,
                "MCP HTTP server does not expose a usable SSE listener endpoint"
            );
            return Ok(false);
        }
        let mut current = Vec::<String>::new();
        let mut line_buffer = String::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            line_buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = line_buffer.find('\n') {
                let line = line_buffer[..pos].trim_end_matches('\r').to_string();
                line_buffer = line_buffer[pos + 1..].to_string();
                if line.is_empty() {
                    self.flush_sse_notification(&mut current);
                    continue;
                }
                if let Some(data) = line.strip_prefix("data:") {
                    current.push(data.strip_prefix(' ').unwrap_or(data).to_string());
                }
            }
        }
        if !current.is_empty() {
            self.flush_sse_notification(&mut current);
        }
        Ok(true)
    }

    fn flush_sse_notification(&self, current: &mut Vec<String>) {
        let data = current.join("\n");
        current.clear();
        if data.trim().is_empty() || data.trim() == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return;
        };
        let Some(notification) = notification_from_value(&value) else {
            return;
        };
        if let Some(handler) = &self.notifications {
            handler(notification);
        }
    }
}

#[cfg(test)]
pub(super) fn parse_http_rpc_response(
    method: &str,
    content_type: Option<&str>,
    body: &str,
) -> anyhow::Result<Option<RpcResponse>> {
    Ok(parse_http_rpc_events(method, content_type, body)?.response)
}

struct RpcEvents {
    response: Option<RpcResponse>,
    notifications: Vec<McpNotification>,
}

fn parse_http_rpc_events(
    method: &str,
    content_type: Option<&str>,
    body: &str,
) -> anyhow::Result<RpcEvents> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(RpcEvents {
            response: None,
            notifications: Vec::new(),
        });
    }
    let looks_like_sse = content_type
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false)
        || body.starts_with("data:")
        || body.starts_with("event:");
    if looks_like_sse {
        let mut notifications = Vec::new();
        for data in sse_data_payloads(body) {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(notification) = notification_from_value(&value) {
                notifications.push(notification);
                continue;
            }
            let response =
                serde_json::from_value::<RpcResponse>(value).with_context(|| {
                    format!("failed to parse MCP HTTP SSE response for {method}")
                })?;
            return Ok(RpcEvents {
                response: Some(response),
                notifications,
            });
        }
        return Ok(RpcEvents {
            response: None,
            notifications,
        });
    }
    let value: Value = serde_json::from_str(body).with_context(|| {
        format!("failed to parse MCP HTTP JSON-RPC response for {method}")
    })?;
    if let Value::Array(items) = value {
        let mut notifications = Vec::new();
        let mut response = None;
        for item in items {
            if let Some(notification) = notification_from_value(&item) {
                notifications.push(notification);
            } else if response.is_none() {
                response = Some(serde_json::from_value(item).with_context(|| {
                    format!("failed to parse MCP HTTP JSON-RPC response for {method}")
                })?);
            }
        }
        return Ok(RpcEvents {
            response,
            notifications,
        });
    }
    let notifications: Vec<McpNotification> =
        notification_from_value(&value).into_iter().collect();
    let response = if notifications.is_empty() {
        Some(
            serde_json::from_value::<RpcResponse>(value).with_context(|| {
                format!("failed to parse MCP HTTP JSON-RPC response for {method}")
            })?,
        )
    } else {
        None
    };
    Ok(RpcEvents {
        response,
        notifications,
    })
}

fn sse_data_payloads(body: &str) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut current = Vec::new();
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !current.is_empty() {
                payloads.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            current.push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }
    if !current.is_empty() {
        payloads.push(current.join("\n"));
    }
    payloads
}

async fn write_json_line(stdin: &mut ChildStdin, value: &Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(super) struct RpcResponse {
    #[serde(default)]
    pub(super) id: Option<u64>,
    #[serde(default)]
    pub(super) result: Option<Value>,
    #[serde(default)]
    pub(super) error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RpcError {
    pub(super) code: i64,
    pub(super) message: String,
}

fn handle_notification_value(
    value: &Value,
    handler: Option<&NotificationHandler>,
) -> bool {
    let Some(notification) = notification_from_value(value) else {
        return false;
    };
    if let Some(handler) = handler {
        handler(notification);
    }
    true
}

fn notification_from_value(value: &Value) -> Option<McpNotification> {
    if value.get("id").is_some() {
        return None;
    }
    let method = value.get("method").and_then(Value::as_str)?.to_string();
    Some(McpNotification {
        method,
        params: value.get("params").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_notifications_before_response() {
        let body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\",\"params\":{\"server\":\"local\"}}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[]}}\n\n"
        );

        let events =
            parse_http_rpc_events("tools/list", Some("text/event-stream"), body).unwrap();

        assert_eq!(events.notifications.len(), 1);
        assert_eq!(
            events.notifications[0].method,
            "notifications/tools/list_changed"
        );
        assert_eq!(events.notifications[0].params, json!({ "server": "local" }));
        let response = events.response.unwrap();
        assert_eq!(response.id, Some(7));
        assert_eq!(response.result.unwrap(), json!({ "tools": [] }));
    }

    #[test]
    fn parses_notification_only_json_body_without_response() {
        let events = parse_http_rpc_events(
            "notifications/tools/list_changed",
            Some("application/json"),
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}",
        )
        .unwrap();

        assert_eq!(
            events.response.as_ref().and_then(|response| response.id),
            None
        );
        assert_eq!(events.notifications.len(), 1);
        assert_eq!(
            events.notifications[0].method,
            "notifications/tools/list_changed"
        );
    }
}
