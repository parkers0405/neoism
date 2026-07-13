use std::collections::BTreeMap;

use anyhow::Context;
use neoism_agent_core::{AuthInfo, ProviderGenerationRequest, ProviderStreamEvent};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth_store::AuthStore;
use crate::provider_error::ProviderError;
use crate::provider_responses::{responses_request_body, ResponsesSseParser};

use super::provider_chat_completion::{
    chat_completion_messages, chat_completion_tools, reasoning_effort,
};
use super::provider_openai_stream::{
    estimate_tokens, finish_open_tool_calls, handle_tool_call_deltas, neoism_user_agent,
    openai_key_with_fallback, parse_stream_line,
};
use super::{ProviderEventStream, ProviderRuntime};

const CODEX_RESPONSES_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_OAUTH_ISSUER: &str = "https://auth.openai.com";
const OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OAUTH_REFRESH_MARGIN_MS: u64 = 60_000;

#[derive(Clone)]
pub(super) struct OpenAiClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Clone)]
pub(super) struct OpenAiRuntime {
    pub(super) client: OpenAiClient,
    pub(super) auth: Option<AuthInfo>,
    pub(super) auth_store: AuthStore,
    pub(super) use_oauth_responses: bool,
    pub(super) allow_openai_env_fallback: bool,
}

impl OpenAiClient {
    pub(super) fn from_env() -> Self {
        let base_url = std::env::var("NEOISM_AGENT_OPENAI_BASE_URL")
            .or_else(|_| std::env::var("OPENAI_BASE_URL"))
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub(super) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

impl ProviderRuntime for OpenAiRuntime {
    fn stream(&self, request: ProviderGenerationRequest) -> ProviderEventStream {
        let request = crate::provider_transform::normalize_request(request);
        let client = self.client.clone();
        let auth = self.auth.clone();
        let auth_store = self.auth_store.clone();
        let allow_openai_env_fallback = self.allow_openai_env_fallback;
        if self.use_oauth_responses && matches!(auth, Some(AuthInfo::OAuth { .. })) {
            return openai_oauth_responses_stream(client, auth_store, auth, request);
        }
        // API-key path for the gpt-5.6 family: chat completions rejects
        // reasoning_effort together with function tools for these models, so
        // route them through the platform Responses API instead.
        if self.use_oauth_responses && request.model_id.contains("gpt-5.6") {
            if let Some(api_key) =
                openai_key_with_fallback(auth.as_ref(), allow_openai_env_fallback)
            {
                return openai_api_key_responses_stream(client, api_key, request);
            }
        }
        let provider_id = request.provider_id.clone();
        Box::pin(async_stream::try_stream! {
            // Refresh an expiring OAuth token (e.g. xAI Grok) before use so a
            // long-lived session doesn't start failing an hour after sign-in.
            let auth = crate::provider_auth::refresh_oauth_if_needed(
                &provider_id,
                auth,
                &auth_store,
                &client.client,
            )
            .await?;
            let api_key = openai_key_with_fallback(auth.as_ref(), allow_openai_env_fallback).ok_or_else(|| {
                anyhow::anyhow!(
                    "OpenAI-compatible provider requested but no API key was found in stored auth or provider environment variables"
                )
            })?;

            yield ProviderStreamEvent::Start;
            yield ProviderStreamEvent::StartStep;

            let messages = chat_completion_messages(&request.messages);
            let mut body = json!({
                "model": request.api.as_ref().map(|api| api.id.as_str()).unwrap_or(&request.model_id),
                "messages": messages,
                "stream": true,
                "stream_options": { "include_usage": true },
            });
            let tools = chat_completion_tools(&request.model_id, &request.tools);
            if !tools.is_empty() {
                body["tools"] = Value::Array(tools);
                body["tool_choice"] = Value::String("auto".to_string());
            }
            if request
                .api
                .as_ref()
                .is_none_or(|api| api.npm != "@openrouter/ai-sdk-provider")
            {
                if let Some(effort) = reasoning_effort(request.variant.as_deref()) {
                    body["reasoning_effort"] = Value::String(effort.to_string());
                }
            }
            if let Some(temperature) = crate::provider_transform::model_temperature(&request.model_id) {
                body["temperature"] = json!(temperature);
            }
            if let Some(top_p) = crate::provider_transform::model_top_p(&request.model_id) {
                body["top_p"] = json!(top_p);
            }
            crate::provider_transform::apply_openai_compatible_request_quirks(&request, &mut body);
            merge_provider_options(&mut body, &request.options);

            let mut http = client
                .client
                .post(format!("{}/chat/completions", client.base_url))
                .bearer_auth(api_key);
            for (name, value) in &request.headers {
                http = http.header(name, value);
            }
            let response = http
                .json(&body)
                .send()
                .await
                .context("failed to send OpenAI-compatible streaming chat completion request")?;

            let status = response.status();
            let mut response = if status.is_success() {
                response
            } else {
                let headers = response.headers().clone();
                let body = response.text().await.unwrap_or_default();
                Err::<reqwest::Response, anyhow::Error>(
                    ProviderError::from_response("OpenAI-compatible", status, &headers, body)
                        .into(),
                )?
            };

            let mut finish = None;
            let mut total_tokens = None;
            let mut input_tokens = None;
            let mut output_tokens = None;
            let mut reasoning_tokens = None;
            let mut cache_read_tokens = None;
            let mut cache_write_tokens = None;
            let mut line = Vec::new();
            let mut done = false;
            let mut text_started = false;
            let mut reasoning_started = false;
            let mut output_text = String::new();
            let mut tool_calls = BTreeMap::new();

            while let Some(chunk) = response.chunk().await? {
                for byte in chunk {
                    if byte == b'\n' {
                        let parsed = parse_stream_line(&line)?;
                        line.clear();
                        if parsed.done {
                            done = true;
                            break;
                        }
                        for delta in parsed.deltas {
                            if !text_started {
                                yield ProviderStreamEvent::TextStart { id: "text".to_string() };
                                text_started = true;
                            }
                            output_text.push_str(&delta);
                            yield ProviderStreamEvent::TextDelta {
                                id: "text".to_string(),
                                delta,
                            };
                        }
                        for delta in parsed.reasoning_deltas {
                            if !reasoning_started {
                                yield ProviderStreamEvent::ReasoningStart { id: "reasoning".to_string() };
                                reasoning_started = true;
                            }
                            yield ProviderStreamEvent::ReasoningDelta {
                                id: "reasoning".to_string(),
                                delta,
                            };
                        }
                        for event in handle_tool_call_deltas(&mut tool_calls, parsed.tool_calls)? {
                            yield event;
                        }
                        if let Some(value) = parsed.finish {
                            finish = Some(value);
                        }
                        if let Some(value) = parsed.total_tokens {
                            total_tokens = Some(value);
                        }
                        if let Some(value) = parsed.input_tokens {
                            input_tokens = Some(value);
                        }
                        if let Some(value) = parsed.output_tokens {
                            output_tokens = Some(value);
                        }
                        if let Some(value) = parsed.reasoning_tokens {
                            reasoning_tokens = Some(value);
                        }
                        if let Some(value) = parsed.cache_read_tokens {
                            cache_read_tokens = Some(value);
                        }
                        if let Some(value) = parsed.cache_write_tokens {
                            cache_write_tokens = Some(value);
                        }
                    } else {
                        line.push(byte);
                    }
                }
                if done {
                    break;
                }
            }

            if !line.is_empty() && !done {
                let parsed = parse_stream_line(&line)?;
                for delta in parsed.deltas {
                    if !text_started {
                        yield ProviderStreamEvent::TextStart { id: "text".to_string() };
                        text_started = true;
                    }
                    output_text.push_str(&delta);
                    yield ProviderStreamEvent::TextDelta {
                        id: "text".to_string(),
                        delta,
                    };
                }
                for delta in parsed.reasoning_deltas {
                    if !reasoning_started {
                        yield ProviderStreamEvent::ReasoningStart { id: "reasoning".to_string() };
                        reasoning_started = true;
                    }
                    yield ProviderStreamEvent::ReasoningDelta {
                        id: "reasoning".to_string(),
                        delta,
                    };
                }
                for event in handle_tool_call_deltas(&mut tool_calls, parsed.tool_calls)? {
                    yield event;
                }
                if let Some(value) = parsed.finish {
                    finish = Some(value);
                }
                if let Some(value) = parsed.total_tokens {
                    total_tokens = Some(value);
                }
                if let Some(value) = parsed.input_tokens {
                    input_tokens = Some(value);
                }
                if let Some(value) = parsed.output_tokens {
                    output_tokens = Some(value);
                }
                if let Some(value) = parsed.reasoning_tokens {
                    reasoning_tokens = Some(value);
                }
                if let Some(value) = parsed.cache_read_tokens {
                    cache_read_tokens = Some(value);
                }
                if let Some(value) = parsed.cache_write_tokens {
                    cache_write_tokens = Some(value);
                }
            }

            if text_started {
                yield ProviderStreamEvent::TextEnd { id: "text".to_string() };
            }
            if reasoning_started {
                yield ProviderStreamEvent::ReasoningEnd { id: "reasoning".to_string() };
            }
            for event in finish_open_tool_calls(&mut tool_calls) {
                yield event;
            }
            let finish = finish.or_else(|| Some("stop".to_string()));
            let input_tokens = input_tokens.unwrap_or_else(|| {
                request
                    .messages
                    .iter()
                    .map(|message| estimate_tokens(&message.content))
                    .sum()
            });
            let output_tokens = output_tokens.unwrap_or_else(|| estimate_tokens(&output_text));
            let reasoning_tokens = reasoning_tokens.unwrap_or(0);
            let cache_read_tokens = cache_read_tokens.unwrap_or(0);
            let cache_write_tokens = cache_write_tokens.unwrap_or(0);
            yield ProviderStreamEvent::FinishStep {
                finish: finish.clone(),
                total_tokens,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                cache_read_tokens,
                cache_write_tokens,
            };
            yield ProviderStreamEvent::Finish {
                finish,
                total_tokens,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                cache_read_tokens,
                cache_write_tokens,
            };
        })
    }
}

fn merge_provider_options(body: &mut Value, options: &BTreeMap<String, Value>) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    for (key, value) in options {
        if matches!(
            key.as_str(),
            "model"
                | "messages"
                | "input"
                | "instructions"
                | "stream"
                | "stream_options"
                | "store"
                | "tools"
                | "tool_choice"
        ) {
            continue;
        }
        object.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_options_merge_without_overriding_structural_fields() {
        let mut body = json!({
            "model": "gpt-test",
            "messages": [],
            "stream": true,
            "temperature": 0.2
        });
        let options = BTreeMap::from([
            ("temperature".to_string(), json!(0.7)),
            ("reasoning_effort".to_string(), json!("high")),
            ("model".to_string(), json!("malicious")),
            ("stream".to_string(), json!(false)),
        ]);

        merge_provider_options(&mut body, &options);

        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["reasoning_effort"], "high");
    }
}

// Platform Responses API with a plain API key — the wire format the gpt-5.6
// family requires when combining function tools with reasoning effort.
fn openai_api_key_responses_stream(
    client: OpenAiClient,
    api_key: String,
    request: ProviderGenerationRequest,
) -> ProviderEventStream {
    Box::pin(async_stream::try_stream! {
        yield ProviderStreamEvent::Start;
        yield ProviderStreamEvent::StartStep;

        let endpoint = std::env::var("NEOISM_AGENT_OPENAI_RESPONSES_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1/responses".to_string());
        let mut body = responses_request_body(
            request.model_id.clone(),
            request.variant.as_deref(),
            &request.messages,
            &request.tools,
        );
        merge_provider_options(&mut body, &request.options);
        let mut request_builder = client
            .client
            .post(&endpoint)
            .bearer_auth(api_key)
            .header("accept", "text/event-stream")
            .header("user-agent", neoism_user_agent())
            .json(&body);
        for (name, value) in &request.headers {
            request_builder = request_builder.header(name, value);
        }

        let response = request_builder
            .send()
            .await
            .context("failed to send OpenAI Responses streaming request")?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut response = if status.is_success() {
            response
        } else {
            let body = response.text().await.unwrap_or_default();
            Err::<reqwest::Response, anyhow::Error>(
                ProviderError::from_response("OpenAI Responses", status, &headers, body).into(),
            )?
        };

        let mut parser = ResponsesSseParser::default();
        let mut line = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            for byte in chunk {
                if byte == b'\n' {
                    let line_text = std::str::from_utf8(&line)?.to_string();
                    line.clear();
                    for event in parser.push_line(&line_text)? {
                        yield event;
                    }
                } else {
                    line.push(byte);
                }
            }
        }
        if !line.is_empty() {
            let line_text = std::str::from_utf8(&line)?.to_string();
            for event in parser.push_line(&line_text)? {
                yield event;
            }
        }
    })
}

fn openai_oauth_responses_stream(
    client: OpenAiClient,
    auth_store: AuthStore,
    auth: Option<AuthInfo>,
    request: ProviderGenerationRequest,
) -> ProviderEventStream {
    Box::pin(async_stream::try_stream! {
        let (access, account_id) = match auth {
            Some(AuthInfo::OAuth {
                refresh,
                access,
                expires,
                account_id,
                enterprise_url,
                ..
            }) => {
                if should_refresh_oauth(expires) {
                    let refreshed = refresh_openai_oauth(&client.client, &refresh, account_id.clone(), enterprise_url.clone()).await?;
                    auth_store.set("openai", refreshed.clone())?;
                    match refreshed {
                        AuthInfo::OAuth { access, account_id, .. } => (access, account_id),
                        _ => unreachable!("refresh_openai_oauth returns OAuth auth"),
                    }
                } else {
                    (access, account_id)
                }
            }
            _ => Err(anyhow::anyhow!(
                "OpenAI OAuth Responses stream requested without OAuth credentials"
            ))?,
        };

        yield ProviderStreamEvent::Start;
        yield ProviderStreamEvent::StartStep;

        let endpoint = std::env::var("NEOISM_AGENT_OPENAI_CODEX_RESPONSES_URL")
            .unwrap_or_else(|_| CODEX_RESPONSES_ENDPOINT.to_string());
        let mut body = responses_request_body(
            request.model_id.clone(),
            request.variant.as_deref(),
            &request.messages,
            &request.tools,
        );
        merge_provider_options(&mut body, &request.options);
        let mut request_builder = client
            .client
            .post(&endpoint)
            .bearer_auth(access)
            .header("accept", "text/event-stream")
            .header("originator", "neoism")
            .header("user-agent", neoism_user_agent())
            .json(&body);
        for (name, value) in &request.headers {
            request_builder = request_builder.header(name, value);
        }
        if let Some(account_id) = account_id {
            request_builder = request_builder.header("ChatGPT-Account-Id", account_id);
        }

        let response = request_builder
            .send()
            .await
            .context("failed to send OpenAI OAuth Responses streaming request")?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut response = if status.is_success() {
            response
        } else {
            let body = response.text().await.unwrap_or_default();
            Err::<reqwest::Response, anyhow::Error>(
                ProviderError::from_response("OpenAI OAuth Responses", status, &headers, body)
                    .into(),
            )?
        };

        let mut parser = ResponsesSseParser::default();
        let mut line = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            for byte in chunk {
                if byte == b'\n' {
                    let line_text = std::str::from_utf8(&line)?.to_string();
                    line.clear();
                    for event in parser.push_line(&line_text)? {
                        yield event;
                    }
                } else {
                    line.push(byte);
                }
            }
        }
        if !line.is_empty() {
            let line_text = std::str::from_utf8(&line)?.to_string();
            for event in parser.push_line(&line_text)? {
                yield event;
            }
        }
    })
}

#[derive(Debug, Deserialize)]
struct OpenAiOAuthRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
}

async fn refresh_openai_oauth(
    client: &reqwest::Client,
    refresh_token: &str,
    previous_account_id: Option<String>,
    enterprise_url: Option<String>,
) -> anyhow::Result<AuthInfo> {
    let issuer = std::env::var("NEOISM_AGENT_OPENAI_OAUTH_ISSUER")
        .unwrap_or_else(|_| OPENAI_OAUTH_ISSUER.to_string());
    // OpenAI's /oauth/token endpoint expects the refresh_token grant as JSON
    // (the authorization_code grant uses form-encoding, but refresh does not).
    // This mirrors codex's `request_chatgpt_token_refresh`; sending this body
    // form-encoded is rejected, which silently breaks auto-refresh.
    let response = client
        .post(format!("{issuer}/oauth/token"))
        .header("content-type", "application/json")
        .json(&json!({
            "client_id": OPENAI_OAUTH_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<OpenAiOAuthRefreshResponse>()
        .await?;
    let account_id = response
        .id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&response.access_token))
        .or(previous_account_id);
    Ok(AuthInfo::OAuth {
        refresh: response
            .refresh_token
            .unwrap_or_else(|| refresh_token.to_string()),
        access: response.access_token,
        expires: now_millis()
            .saturating_add(response.expires_in.unwrap_or(3_600) * 1_000),
        account_id,
        enterprise_url,
    })
}

fn should_refresh_oauth(expires: u64) -> bool {
    expires != 0 && expires <= now_millis().saturating_add(OAUTH_REFRESH_MARGIN_MS)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    use base64::Engine;

    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("chatgpt_account_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|value| value.get("chatgpt_account_id"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("id"))
                .and_then(serde_json::Value::as_str)
        })
        .map(ToOwned::to_owned)
}
