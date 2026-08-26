use anyhow::Context;
use neoism_agent_core::{CreateSessionRequest, ModelRef, UserModel};
use serde_json::{json, Value};

use crate::chat_render::ChatRenderState;
use crate::chat_ui::print_user_prompt;
use crate::{provider_id, response_json, split_model_ref, DIM, RESET};

pub(crate) async fn create_cli_session(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
    model: Option<&str>,
    agent: Option<&str>,
    variant: Option<&str>,
) -> anyhow::Result<String> {
    let request = CreateSessionRequest {
        parent_id: None,
        title: None,
        agent: agent.map(ToOwned::to_owned),
        model: model
            .map(|model| model_ref_from_cli_model(model, variant))
            .transpose()?,
        permission: None,
        workspace_id: None,
    };
    let mut builder = client.post(format!("{server}/v2/sessions")).json(&request);
    if let Some(dir) = dir {
        builder = builder.query(&[("directory", dir)]);
    }
    let value = response_json(builder.send().await?).await?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .context("server did not return session id")
}

pub(crate) async fn persist_session_model(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    model: &str,
    variant: Option<&str>,
) -> anyhow::Result<()> {
    let model = model_ref_from_cli_model(model, variant)?;
    let _: Value = response_json(
        client
            .patch(format!("{server}/v2/sessions/{session_id}"))
            .json(&json!({ "model": model }))
            .send()
            .await?,
    )
    .await?;
    Ok(())
}

pub(crate) fn normalize_model_ref(model: &str, default_provider: &str) -> String {
    if split_model_ref(model).is_some() {
        model.to_string()
    } else {
        format!("{default_provider}/{model}")
    }
}

pub(crate) fn user_model_from_cli_model(
    model: &str,
    variant: Option<&str>,
) -> anyhow::Result<UserModel> {
    let (provider_id, model_id) = split_model_ref(model)
        .with_context(|| format!("model must use provider/model form: {model}"))?;
    Ok(UserModel {
        provider_id,
        model_id,
        variant: variant.map(ToOwned::to_owned),
    })
}

fn model_ref_from_cli_model(
    model: &str,
    variant: Option<&str>,
) -> anyhow::Result<ModelRef> {
    let (provider_id, model_id) = split_model_ref(model)
        .with_context(|| format!("model must use provider/model form: {model}"))?;
    Ok(ModelRef {
        id: model_id,
        provider_id,
        variant: variant.map(ToOwned::to_owned),
    })
}

pub(crate) async fn print_provider_model_list(
    client: &reqwest::Client,
    server: &str,
    provider: &str,
    current_model: Option<&str>,
) -> anyhow::Result<()> {
    let value =
        response_json(client.get(format!("{server}/v2/providers")).send().await?).await?;
    let providers = value
        .get("all")
        .and_then(Value::as_array)
        .context("server did not return provider list")?;
    let provider = providers
        .iter()
        .find(|item| provider_id(item) == provider)
        .with_context(|| format!("provider not found: {provider}"))?;
    let provider_id = provider_id(provider);
    let models = provider
        .get("models")
        .and_then(Value::as_object)
        .context("provider did not include models")?;
    let mut model_ids = models.keys().collect::<Vec<_>>();
    model_ids.sort();
    for model_id in model_ids {
        let full = format!("{provider_id}/{model_id}");
        if current_model == Some(full.as_str()) {
            println!("* {full}");
        } else {
            println!("  {full}");
        }
    }
    Ok(())
}

pub(crate) async fn print_session_messages(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let path = match limit {
        Some(limit) => {
            format!("{server}/v2/sessions/{session_id}/messages?order=desc&limit={limit}")
        }
        None => format!("{server}/v2/sessions/{session_id}/messages"),
    };
    let value = response_json(client.get(path).send().await?).await?;
    let mut messages = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .context("messages response was not a list")?;
    if limit.is_some() {
        messages.reverse();
    }
    for message in messages {
        let role = message
            .get("info")
            .and_then(|info| info.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("message");
        if let Some(text) = message_text(&message) {
            println!("{role}: {text}");
        }
    }
    Ok(())
}

pub(crate) async fn print_session_replay(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    limit: usize,
) -> anyhow::Result<()> {
    let value = response_json(
        client
            .get(format!(
                "{server}/v2/sessions/{session_id}/messages?order=desc&limit={limit}"
            ))
            .send()
            .await?,
    )
    .await?;
    let mut messages = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .context("messages response was not a list")?;
    messages.reverse();
    for message in messages {
        match message
            .get("info")
            .and_then(|info| info.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("message")
        {
            "user" => {
                if let Some(text) = message_text(&message) {
                    print_user_prompt(&text);
                }
            }
            "assistant" => print_assistant_replay(&message)?,
            _ => {}
        }
    }
    Ok(())
}

fn print_assistant_replay(message: &Value) -> anyhow::Result<()> {
    let mut render_state = ChatRenderState::default();
    let mut reasoning_chars = 0usize;
    if let Some(parts) = message.get("parts").and_then(Value::as_array) {
        for part in parts {
            match part.get("type").and_then(Value::as_str).unwrap_or_default() {
                "text" => {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        render_state.text_delta(text)?;
                    }
                }
                "tool" => {
                    render_state.part_updated(part)?;
                }
                "reasoning" => {
                    reasoning_chars = reasoning_chars.saturating_add(
                        part.get("text")
                            .and_then(Value::as_str)
                            .map(str::len)
                            .unwrap_or(0),
                    );
                }
                _ => {}
            }
        }
    }
    render_state.finish()?;
    if reasoning_chars > 0 {
        println!("  {DIM}✻ Thinking saved · {reasoning_chars} chars{RESET}");
    }
    Ok(())
}

pub(crate) async fn fetch_session_messages(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<Vec<Value>> {
    let value = response_json(
        client
            .get(format!("{server}/v2/sessions/{session_id}/messages"))
            .send()
            .await?,
    )
    .await?;
    value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .context("messages response was not a list")
}

pub(crate) async fn fetch_context_usage_label(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<Option<String>> {
    let messages = response_json(
        client
            .get(format!(
                "{server}/v2/sessions/{session_id}/messages?order=desc&limit=200"
            ))
            .send()
            .await?,
    )
    .await?;
    let providers =
        response_json(client.get(format!("{server}/v2/providers")).send().await?).await?;
    let Some(messages) = messages.get("items").and_then(Value::as_array) else {
        return Ok(None);
    };
    Ok(context_usage_label(messages, &providers))
}

pub(crate) fn context_usage_label(
    messages: &[Value],
    providers: &Value,
) -> Option<String> {
    let assistant = latest_assistant_with_tokens(messages)?;
    let info = assistant.get("info")?;
    let provider_id = info.get("providerId").and_then(Value::as_str)?;
    let model_id = info.get("modelId").and_then(Value::as_str)?;
    let tokens = info.get("tokens")?;
    let total = token_total(tokens);
    let limit = model_context_limit(providers, provider_id, model_id)?;
    if limit == 0 {
        return None;
    }
    let percent =
        ((u128::from(total) * 100) + (u128::from(limit) / 2)) / u128::from(limit);
    Some(format!(
        "ctx {} {}% {}/{}",
        compact_model_label(model_id),
        percent,
        format_token_count(total),
        format_token_count(limit)
    ))
}

fn latest_assistant_with_tokens(messages: &[Value]) -> Option<&Value> {
    messages
        .iter()
        .filter(|message| {
            message
                .get("info")
                .and_then(|info| info.get("role"))
                .and_then(Value::as_str)
                == Some("assistant")
        })
        .filter(|message| {
            message
                .get("info")
                .and_then(|info| info.get("tokens"))
                .is_some()
        })
        .max_by_key(|message| {
            let info = message.get("info").unwrap_or(&Value::Null);
            info.get("time")
                .and_then(|time| time.get("completed").or_else(|| time.get("created")))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
}

fn token_total(tokens: &Value) -> u64 {
    // The visible context metric sums the normalized token buckets.
    tokens
        .get("input")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(tokens.get("output").and_then(Value::as_u64).unwrap_or(0))
        .saturating_add(tokens.get("reasoning").and_then(Value::as_u64).unwrap_or(0))
        .saturating_add(
            tokens
                .get("cache")
                .and_then(|cache| cache.get("read"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
        .saturating_add(
            tokens
                .get("cache")
                .and_then(|cache| cache.get("write"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
}

fn model_context_limit(
    providers: &Value,
    provider_id: &str,
    model_id: &str,
) -> Option<u64> {
    let providers = providers.get("all").and_then(Value::as_array)?;
    let provider = providers
        .iter()
        .find(|provider| provider_id_value(provider) == provider_id)?;
    let models = provider.get("models").and_then(Value::as_object)?;
    models
        .get(model_id)
        .or_else(|| {
            models.values().find(|model| {
                model
                    .get("api")
                    .and_then(|api| api.get("id"))
                    .and_then(Value::as_str)
                    == Some(model_id)
            })
        })?
        .get("limit")
        .and_then(model_limit_for_usage)
}

fn model_limit_for_usage(limit: &Value) -> Option<u64> {
    limit
        .get("input")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .or_else(|| {
            limit
                .get("context")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
        })
}

fn provider_id_value(provider: &Value) -> &str {
    provider
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn compact_model_label(model_id: &str) -> &str {
    model_id.rsplit('/').next().unwrap_or(model_id)
}

fn format_token_count(value: u64) -> String {
    if value >= 1_000_000 {
        let whole = value / 1_000_000;
        let decimal = (value % 1_000_000) / 100_000;
        if decimal == 0 {
            format!("{whole}m")
        } else {
            format!("{whole}.{decimal}m")
        }
    } else if value >= 1_000 {
        format!("{}k", (value + 500) / 1_000)
    } else {
        value.to_string()
    }
}

pub(crate) async fn fetch_session_undo_tree(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<Value> {
    response_json(
        client
            .get(format!("{server}/v2/sessions/{session_id}/undo-tree"))
            .send()
            .await?,
    )
    .await
}

pub(crate) async fn fetch_session_queue(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<Value> {
    response_json(
        client
            .get(format!("{server}/v2/sessions/{session_id}/queue"))
            .send()
            .await?,
    )
    .await
}

pub(crate) async fn fetch_permission_requests(
    client: &reqwest::Client,
    server: &str,
) -> anyhow::Result<Value> {
    response_json(client.get(format!("{server}/v2/interactions/permissions")).send().await?).await
}

pub(crate) async fn fetch_question_requests(
    client: &reqwest::Client,
    server: &str,
) -> anyhow::Result<Value> {
    response_json(client.get(format!("{server}/v2/interactions/questions")).send().await?).await
}

pub(crate) async fn abort_session_if_busy(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let statuses = response_json(
        client
            .get(format!("{server}/v2/sessions/status"))
            .send()
            .await?,
    )
    .await
    .unwrap_or(Value::Null);
    let busy = statuses
        .get(session_id)
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|status| status != "idle");
    if busy {
        let _ = client
            .post(format!("{server}/v2/sessions/{session_id}/abort"))
            .send()
            .await;
    }
    Ok(())
}

pub(crate) fn undo_cursor_message_id(tree: &Value) -> Option<String> {
    tree.get("cursor")
        .and_then(|cursor| cursor.get("messageID").or_else(|| cursor.get("messageId")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub(crate) fn next_redo_message_id(tree: &Value, revert_id: &str) -> Option<String> {
    let nodes = tree.get("nodes")?.as_array()?;
    let start = nodes.iter().position(|node| {
        node.get("messageID")
            .or_else(|| node.get("messageId"))
            .and_then(Value::as_str)
            == Some(revert_id)
    })?;
    nodes.iter().skip(start + 1).find_map(|node| {
        let status = node.get("status").and_then(Value::as_str)?;
        if status != "reverted" {
            return None;
        }
        node.get("messageID")
            .or_else(|| node.get("messageId"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

pub(crate) async fn latest_assistant_text(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<Option<String>> {
    let value = response_json(
        client
            .get(format!("{server}/v2/sessions/{session_id}/messages"))
            .send()
            .await?,
    )
    .await?;
    let Some(messages) = value.get("items").and_then(Value::as_array) else {
        return Ok(None);
    };
    Ok(messages
        .iter()
        .rev()
        .find(|message| {
            message
                .get("info")
                .and_then(|info| info.get("role"))
                .and_then(Value::as_str)
                == Some("assistant")
        })
        .and_then(message_text))
}

pub(crate) fn message_info_id(message: &Value) -> Option<String> {
    message
        .get("info")
        .and_then(|info| info.get("id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub(crate) fn message_text(message: &Value) -> Option<String> {
    let text = message
        .get("parts")?
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) async fn ensure_success_response(
    response: reqwest::Response,
) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let url = response.url().clone();
    let text = response.text().await.unwrap_or_default();
    anyhow::bail!("HTTP {status} for {url}: {text}");
}

pub(crate) async fn ensure_empty_response(
    response: reqwest::Response,
) -> anyhow::Result<()> {
    let status = response.status();
    let url = response.url().clone();
    let text = response
        .text()
        .await
        .with_context(|| format!("failed to read response body from {url}"))?;
    if !status.is_success() {
        anyhow::bail!("HTTP {status} for {url}: {text}");
    }
    Ok(())
}
