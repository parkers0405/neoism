use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::Engine;
use futures::StreamExt;
use neoism_agent_core::{AuthInfo, ProviderApiInfo};
use neoism_agent_plugin_api::{GeneratedMedia, MediaGenerationRequest, MediaKind};
use serde_json::{json, Value};

use crate::auth_store::AuthStore;

const OPENAI_API_ROOT: &str = "https://api.openai.com/v1";
const OPENAI_CODEX_IMAGE_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/codex/images/generations";
const XAI_API_ROOT: &str = "https://api.x.ai/v1";
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
const MAX_VIDEO_BYTES: usize = 250 * 1024 * 1024;
const VIDEO_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub(crate) async fn generate_media(
    auth_store: &AuthStore,
    api: Option<&ProviderApiInfo>,
    request: MediaGenerationRequest,
) -> anyhow::Result<GeneratedMedia> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;
    let auth = auth_store
        .get(&request.provider_id)
        .await?
        .with_context(|| {
            format!(
                "{} is not connected; run /connect and connect {}",
                request.provider_id, request.provider_id
            )
        })?;
    match (request.provider_id.as_str(), request.kind) {
        ("openai", MediaKind::Image) => {
            generate_openai_image(&client, auth_store, auth, api, &request).await
        }
        ("openai", MediaKind::Video) => anyhow::bail!(
            "OpenAI video generation is unavailable; configure agent.videoModel with an xAI video model"
        ),
        ("xai", MediaKind::Image) => {
            let auth = crate::provider_auth::refresh_oauth_if_needed(
                "xai",
                Some(auth),
                auth_store,
                &client,
            )
            .await?
            .context("xAI connection is unavailable")?;
            generate_xai_image(&client, auth, api, &request).await
        }
        ("xai", MediaKind::Video) => {
            let auth = crate::provider_auth::refresh_oauth_if_needed(
                "xai",
                Some(auth),
                auth_store,
                &client,
            )
            .await?
            .context("xAI connection is unavailable")?;
            generate_xai_video(&client, auth, api, &request).await
        }
        (provider, _) => anyhow::bail!(
            "media generation is not implemented for provider {provider}; use openai for images or xai for images and video"
        ),
    }
}

async fn generate_openai_image(
    client: &reqwest::Client,
    auth_store: &AuthStore,
    auth: AuthInfo,
    api: Option<&ProviderApiInfo>,
    request: &MediaGenerationRequest,
) -> anyhow::Result<GeneratedMedia> {
    let (endpoint, token, account_id) = match auth {
        oauth @ AuthInfo::OAuth { .. } => {
            let (token, account_id) = super::provider_openai::openai_oauth_credentials(
                client, auth_store, oauth,
            )
            .await?;
            (OPENAI_CODEX_IMAGE_ENDPOINT.to_string(), token, account_id)
        }
        AuthInfo::Api { key, .. } => (
            format!("{}/images/generations", api_root(api, OPENAI_API_ROOT)),
            key,
            None,
        ),
        AuthInfo::WellKnown { token, .. } => (
            format!("{}/images/generations", api_root(api, OPENAI_API_ROOT)),
            token,
            None,
        ),
    };
    let mut body = json!({
        "model": request.model_id,
        "prompt": request.prompt,
        "n": 1,
    });
    copy_option(&mut body, &request.options, "size");
    copy_option(&mut body, &request.options, "quality");
    copy_option(&mut body, &request.options, "background");
    let mut http = client
        .post(endpoint)
        .bearer_auth(token)
        .header("originator", "neoism")
        .json(&body);
    if let Some(account_id) = account_id {
        http = http.header("chatgpt-account-id", account_id);
    }
    let response = checked_json(
        send_cancellable(http, request.cancel.as_ref(), "OpenAI image generation")
            .await?,
        "OpenAI image generation",
    )
    .await?;
    media_from_image_response(client, response, request.cancel.as_ref()).await
}

async fn generate_xai_image(
    client: &reqwest::Client,
    auth: AuthInfo,
    api: Option<&ProviderApiInfo>,
    request: &MediaGenerationRequest,
) -> anyhow::Result<GeneratedMedia> {
    let mut body = json!({
        "model": request.model_id,
        "prompt": request.prompt,
        "n": 1,
        "response_format": "b64_json",
    });
    copy_option(&mut body, &request.options, "aspect_ratio");
    copy_option(&mut body, &request.options, "resolution");
    copy_option(&mut body, &request.options, "quality");
    let response = checked_json(
        send_cancellable(
            client
                .post(format!(
                    "{}/images/generations",
                    api_root(api, XAI_API_ROOT)
                ))
                .bearer_auth(bearer(&auth))
                .json(&body),
            request.cancel.as_ref(),
            "xAI image generation",
        )
        .await?,
        "xAI image generation",
    )
    .await?;
    media_from_image_response(client, response, request.cancel.as_ref()).await
}

async fn generate_xai_video(
    client: &reqwest::Client,
    auth: AuthInfo,
    api: Option<&ProviderApiInfo>,
    request: &MediaGenerationRequest,
) -> anyhow::Result<GeneratedMedia> {
    let root = api_root(api, XAI_API_ROOT);
    let token = bearer(&auth).to_string();
    let mut body = json!({
        "model": request.model_id,
        "prompt": request.prompt,
    });
    copy_option(&mut body, &request.options, "duration");
    copy_option(&mut body, &request.options, "aspect_ratio");
    copy_option(&mut body, &request.options, "resolution");
    let started = checked_json(
        send_cancellable(
            client
                .post(format!("{root}/videos/generations"))
                .bearer_auth(&token)
                .json(&body),
            request.cancel.as_ref(),
            "xAI video generation",
        )
        .await?,
        "xAI video generation",
    )
    .await?;
    let request_id = started
        .get("request_id")
        .or_else(|| started.get("id"))
        .and_then(Value::as_str)
        .context("xAI video response did not include request_id")?;
    let deadline = Instant::now() + VIDEO_TIMEOUT;
    loop {
        ensure_not_cancelled(request)?;
        if Instant::now() >= deadline {
            anyhow::bail!("xAI video generation timed out after 10 minutes")
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status = checked_json(
            send_cancellable(
                client
                    .get(format!("{root}/videos/{request_id}"))
                    .bearer_auth(&token),
                request.cancel.as_ref(),
                "xAI video status",
            )
            .await?,
            "xAI video status",
        )
        .await?;
        match status
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
        {
            "done" | "completed" | "succeeded" => {
                let url = status
                    .pointer("/video/url")
                    .or_else(|| status.get("video_url"))
                    .and_then(Value::as_str)
                    .context("completed xAI video response did not include a URL")?;
                let bytes = bounded_download(
                    send_cancellable(
                        client.get(url),
                        request.cancel.as_ref(),
                        "xAI video download",
                    )
                    .await?,
                    MAX_VIDEO_BYTES,
                    request.cancel.as_ref(),
                )
                .await?;
                if bytes.get(4..8) != Some(b"ftyp") {
                    anyhow::bail!("xAI video download was not an MP4 file")
                }
                return Ok(GeneratedMedia {
                    bytes,
                    mime: "video/mp4".to_string(),
                    filename: "generated-video.mp4".to_string(),
                    revised_prompt: None,
                });
            }
            "failed" | "expired" | "cancelled" => {
                anyhow::bail!(
                    "xAI video generation {}: {}",
                    status
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("failed"),
                    response_error(&status)
                )
            }
            _ => {}
        }
    }
}

async fn media_from_image_response(
    client: &reqwest::Client,
    response: Value,
    cancel: Option<&Arc<AtomicBool>>,
) -> anyhow::Result<GeneratedMedia> {
    let item = response
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .context("image generation returned no images")?;
    let bytes = if let Some(encoded) = item.get("b64_json").and_then(Value::as_str) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("image generation returned invalid base64")?;
        if bytes.len() > MAX_IMAGE_BYTES {
            anyhow::bail!("generated image exceeds the 25 MiB limit")
        }
        bytes
    } else if let Some(url) = item.get("url").and_then(Value::as_str) {
        bounded_download(
            send_cancellable(client.get(url), cancel, "image download").await?,
            MAX_IMAGE_BYTES,
            cancel,
        )
        .await?
    } else {
        anyhow::bail!("image generation returned neither b64_json nor url")
    };
    let (mime, extension) = image_format(&bytes)
        .context("image generation returned an unsupported image format")?;
    Ok(GeneratedMedia {
        bytes,
        mime: mime.to_string(),
        filename: format!("generated-image.{extension}"),
        revised_prompt: item
            .get("revised_prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

async fn checked_json(
    response: reqwest::Response,
    operation: &str,
) -> anyhow::Result<Value> {
    let status = response.status();
    let bytes = bounded_download(response, 2 * 1024 * 1024, None).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("{operation} returned invalid JSON ({status})"))?;
    if !status.is_success() {
        anyhow::bail!("{operation} failed ({status}): {}", response_error(&value))
    }
    Ok(value)
}

async fn bounded_download(
    response: reqwest::Response,
    limit: usize,
    cancel: Option<&Arc<AtomicBool>>,
) -> anyhow::Result<Vec<u8>> {
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("media download failed ({status})")
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        anyhow::bail!("generated media exceeds the {} byte limit", limit)
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::SeqCst)) {
            anyhow::bail!("media generation cancelled")
        }
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            anyhow::bail!("generated media exceeds the {} byte limit", limit)
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn send_cancellable(
    request: reqwest::RequestBuilder,
    cancel: Option<&Arc<AtomicBool>>,
    operation: &str,
) -> anyhow::Result<reqwest::Response> {
    let send = request.send();
    tokio::pin!(send);
    loop {
        tokio::select! {
            response = &mut send => return response.map_err(Into::into),
            _ = tokio::time::sleep(Duration::from_millis(100)), if cancel.is_some() => {
                if cancel.is_some_and(|cancel| cancel.load(Ordering::SeqCst)) {
                    anyhow::bail!("{operation} cancelled")
                }
            }
        }
    }
}

fn ensure_not_cancelled(request: &MediaGenerationRequest) -> anyhow::Result<()> {
    if request
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.load(Ordering::SeqCst))
    {
        anyhow::bail!("media generation cancelled")
    }
    Ok(())
}

fn bearer(auth: &AuthInfo) -> &str {
    match auth {
        AuthInfo::Api { key, .. } => key,
        AuthInfo::OAuth { access, .. } => access,
        AuthInfo::WellKnown { token, .. } => token,
    }
}

fn api_root<'a>(api: Option<&'a ProviderApiInfo>, fallback: &'a str) -> &'a str {
    api.map(|api| api.url.trim_end_matches('/'))
        .filter(|url| !url.is_empty())
        .unwrap_or(fallback)
}

fn copy_option(
    body: &mut Value,
    options: &std::collections::BTreeMap<String, Value>,
    key: &str,
) {
    if let Some(value) = options.get(key) {
        body[key] = value.clone();
    }
}

fn response_error(value: &Value) -> String {
    value
        .pointer("/error/message")
        .or_else(|| value.get("error"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown provider error")
        .to_string()
}

fn image_format(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::image_format;

    #[test]
    fn image_format_uses_file_signature() {
        assert_eq!(
            image_format(b"\x89PNG\r\n\x1a\nrest"),
            Some(("image/png", "png"))
        );
        assert_eq!(
            image_format(b"\xff\xd8\xffrest"),
            Some(("image/jpeg", "jpg"))
        );
        assert_eq!(
            image_format(b"RIFF0000WEBPrest"),
            Some(("image/webp", "webp"))
        );
        assert_eq!(image_format(b"not an image"), None);
    }
}
