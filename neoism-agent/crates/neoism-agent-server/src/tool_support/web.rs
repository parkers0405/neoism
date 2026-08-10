use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use serde_json::{json, Value};

use super::args::required_string;
use super::{ToolContext, ToolExecutionResult};

const MAX_WEB_BODY_BYTES: usize = 200_000;
const MAX_WEBFETCH_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const MAX_WEBSEARCH_RESPONSE_BYTES: usize = 256 * 1024;

pub(super) async fn webfetch_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let url = required_string(&arguments, "url")?;
    context.ensure_allowed("webfetch", url)?;
    let parsed = parse_web_url(url)?;
    let format = arguments
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("markdown");
    let timeout = arguments
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(30)
        .clamp(1, 120);
    let response = tokio::time::timeout(
        Duration::from_secs(timeout),
        web_client()?
            .get(parsed.clone())
            .header("accept", accept_header(format))
            .header(
                "user-agent",
                format!("neoism-agent/{}", env!("CARGO_PKG_VERSION")),
            )
            .send(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("request timed out after {timeout} seconds"))?
    .with_context(|| format!("failed to fetch {url}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("web fetch returned HTTP {status} for {url}");
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    if let Some(content_type) = content_type.as_deref() {
        let mime = content_type.split(';').next().unwrap_or_default().trim();
        if !is_textual_mime(mime) {
            anyhow::bail!("unsupported fetched content type {mime}");
        }
    }
    let bytes = collect_bounded_response(response, MAX_WEBFETCH_RESPONSE_BYTES)
        .await
        .with_context(|| format!("failed to read response body from {url}"))?;
    let (output, truncated) = render_web_body_as(&bytes, format);

    Ok(ToolExecutionResult {
        title: format!("Fetch {url}"),
        output,
        metadata: Some(json!({
            "url": parsed.as_str(),
            "status": status.as_u16(),
            "contentType": content_type,
            "bytes": bytes.len(),
            "truncated": truncated,
            "format": format,
        })),
    })
}

pub(super) async fn websearch_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let query = required_string(&arguments, "query")?;
    context.ensure_allowed("websearch", query)?;
    let endpoint = std::env::var("NEOISM_AGENT_WEBSEARCH_ENDPOINT")
        .unwrap_or_else(|_| "https://duckduckgo.com/html/".to_string());
    let response = web_client()?
        .get(&endpoint)
        .query(&[("q", query)])
        .header(
            "user-agent",
            format!("neoism-agent/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .with_context(|| format!("failed to search web for {query}"))?;
    if !response.status().is_success() {
        anyhow::bail!("web search provider returned {}", response.status());
    }
    let bytes = collect_bounded_response(response, MAX_WEBSEARCH_RESPONSE_BYTES)
        .await
        .with_context(|| "failed to read web search response")?;
    let (output, truncated) = render_web_body(&bytes);

    Ok(ToolExecutionResult {
        title: format!("Search {query}"),
        output,
        metadata: Some(json!({
            "query": query,
            "endpoint": endpoint,
            "bytes": bytes.len(),
            "truncated": truncated,
        })),
    })
}

fn web_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .with_context(|| "failed to build web client")
}

fn accept_header(format: &str) -> &'static str {
    match format {
        "text" => "text/plain, text/markdown;q=0.9, text/html;q=0.8, */*;q=0.1",
        "html" => "text/html, application/xhtml+xml;q=0.9, text/plain;q=0.8, */*;q=0.1",
        _ => "text/markdown, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1",
    }
}

fn is_textual_mime(mime: &str) -> bool {
    mime.is_empty()
        || mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}

async fn collect_bounded_response(
    response: reqwest::Response,
    maximum_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes as u64)
    {
        anyhow::bail!("response exceeds {maximum_bytes} byte limit");
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(64 * 1024)
            .min(maximum_bytes as u64) as usize,
    );
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > maximum_bytes {
            anyhow::bail!("response exceeds {maximum_bytes} byte limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn render_web_body_as(bytes: &[u8], format: &str) -> (String, bool) {
    if format == "html" {
        let truncated = bytes.len() > MAX_WEB_BODY_BYTES;
        let bytes = &bytes[..bytes.len().min(MAX_WEB_BODY_BYTES)];
        let mut output = String::from_utf8_lossy(bytes).to_string();
        if truncated {
            output.push_str("\n\n(Output truncated at 200 KB.)");
        }
        return (output, truncated);
    }
    render_web_body(bytes)
}

fn parse_web_url(url: &str) -> anyhow::Result<reqwest::Url> {
    let parsed =
        reqwest::Url::parse(url).with_context(|| format!("invalid URL {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("web tools only support http and https URLs");
    }
    Ok(parsed)
}

pub(super) fn render_web_body(bytes: &[u8]) -> (String, bool) {
    let truncated = bytes.len() > MAX_WEB_BODY_BYTES;
    let bytes = &bytes[..bytes.len().min(MAX_WEB_BODY_BYTES)];
    let text = String::from_utf8_lossy(bytes);
    let mut rendered = String::new();
    let mut in_tag = false;
    let mut last_space = false;
    for ch in text.chars() {
        match ch {
            '<' => {
                in_tag = true;
                if !last_space && !rendered.is_empty() {
                    rendered.push(' ');
                    last_space = true;
                }
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            _ if ch.is_whitespace() => {
                if !last_space && !rendered.is_empty() {
                    rendered.push(' ');
                    last_space = true;
                }
            }
            _ => {
                rendered.push(ch);
                last_space = false;
            }
        }
    }
    if truncated {
        rendered.push_str("\n\n(Output truncated at 200 KB.)");
    }
    (rendered.trim().to_string(), truncated)
}
