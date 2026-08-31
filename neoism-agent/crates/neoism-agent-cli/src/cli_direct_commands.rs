use anyhow::Context;
use neoism_agent_core::{CreateSessionRequest, Id, IdKind, PromptPart, PromptRequest, UserModel};
use serde_json::{json, Value};

use crate::{
    normalize_server, print_json, provider_id, request_with_dir, response_json,
    split_model_ref,
};

pub(super) async fn doctor(server: String, dir: Option<String>) -> anyhow::Result<()> {
    let server = normalize_server(&server);
    let client = reqwest::Client::new();
    let health =
        response_json(client.get(format!("{server}/v2/health")).send().await?)
            .await?;
    let providers =
        response_json(client.get(format!("{server}/v2/providers")).send().await?).await?;
    let config_validation = response_json(
        request_with_dir(
            client.get(format!("{server}/v2/config/validate")),
            dir.as_deref(),
        )
        .send()
        .await?,
    )
    .await?;
    let connected = providers
        .get("connected")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    print_json(json!({
        "server": server,
        "health": health,
        "connectedProviders": connected,
        "configValidation": config_validation,
    }))
}

pub(super) async fn models(
    server: String,
    provider: Option<String>,
    verbose: bool,
) -> anyhow::Result<()> {
    let server = normalize_server(&server);
    let value = response_json(
        reqwest::Client::new()
            .get(format!("{server}/v2/providers"))
            .send()
            .await?,
    )
    .await?;
    let all = value
        .get("all")
        .and_then(Value::as_array)
        .context("server did not return provider list")?;
    let mut providers = all.iter().collect::<Vec<_>>();
    providers.sort_by(|left, right| provider_id(left).cmp(&provider_id(right)));

    if let Some(provider) = provider {
        let item = providers
            .into_iter()
            .find(|item| provider_id(item) == provider)
            .with_context(|| format!("provider not found: {provider}"))?;
        print_provider_models(item, verbose)?;
        return Ok(());
    }

    for item in providers {
        print_provider_models(item, verbose)?;
    }
    Ok(())
}

fn print_provider_models(provider: &Value, verbose: bool) -> anyhow::Result<()> {
    let provider_id = provider_id(provider);
    let Some(models) = provider.get("models").and_then(Value::as_object) else {
        return Ok(());
    };
    let mut entries = models.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (model_id, model) in entries {
        println!("{provider_id}/{model_id}");
        if verbose {
            println!("{}", serde_json::to_string_pretty(model)?);
        }
    }
    Ok(())
}

pub(super) async fn run(
    server: String,
    session: Option<String>,
    dir: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    agent: Option<String>,
    variant: Option<String>,
    message: String,
) -> anyhow::Result<()> {
    if message.trim().is_empty() {
        anyhow::bail!("You must provide a message")
    }

    let server = normalize_server(&server);
    let client = reqwest::Client::new();
    let session_id = match session {
        Some(id) => id,
        None => {
            let mut request =
                client
                    .post(format!("{server}/v2/sessions"))
                    .json(&CreateSessionRequest {
                        parent_id: None,
                        title: None,
                        agent: agent.clone(),
                        model: None,
                        permission: None,
                        workspace_id: None,
                    });
            if let Some(dir) = &dir {
                request = request.query(&[("directory", dir)]);
            }
            let value = response_json(request.send().await?).await?;
            value
                .get("id")
                .and_then(|value| value.as_str())
                .context("server did not return session id")?
                .to_string()
        }
    };

    let prompt_model = match (provider, model) {
        (None, None) => None,
        (None, Some(model)) => {
            let (provider_id, model_id) =
                split_model_ref(&model).unwrap_or_else(|| ("openai".to_string(), model));
            Some(UserModel {
                provider_id,
                model_id,
                connection_id: None,
                variant: variant.clone(),
            })
        }
        (provider, model) => Some(UserModel {
            provider_id: provider.unwrap_or_else(|| "openai".to_string()),
            model_id: model.unwrap_or_else(|| "gpt-4.1-mini".to_string()),
            connection_id: None,
            variant: variant.clone(),
        }),
    };

    let user_message_id = Id::ascending(IdKind::Message);
    client
        .post(format!("{server}/v2/sessions/{session_id}/prompt"))
        .json(&PromptRequest {
            message_id: Some(user_message_id.clone()),
            model: prompt_model,
            agent,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text { text: message }],
        })
        .send()
        .await?
        .error_for_status()?;
    client
        .post(format!("{server}/v2/sessions/{session_id}/wait"))
        .send()
        .await?
        .error_for_status()?;
    let user_message_id = user_message_id.to_string();
    let mut cursor = String::new();
    let response = loop {
        let messages = response_json(
            client
                .get(format!("{server}/v2/sessions/{session_id}/messages"))
                .query(&[
                    ("limit", "256"),
                    ("order", "desc"),
                    ("cursor", cursor.as_str()),
                ])
                .send()
                .await?,
        )
        .await?;
        if let Some(response) = assistant_response_to(&messages, &user_message_id) {
            break response.clone();
        }
        let items = messages
            .get("items")
            .and_then(Value::as_array)
            .context("server did not return a message page")?;
        if items.len() < 256 || page_contains_message(items, &user_message_id) {
            anyhow::bail!("submitted prompt completed without an assistant response");
        }
        cursor = items
            .last()
            .and_then(|message| message.get("info"))
            .and_then(|info| info.get("id"))
            .and_then(Value::as_str)
            .context("server returned a message page without a cursor")?
            .to_string();
    };
    if let Some(error) = response
        .get("info")
        .and_then(|info| info.get("error"))
        .filter(|error| !error.is_null())
    {
        anyhow::bail!("submitted prompt failed: {error}");
    }

    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({ "sessionId": session_id, "message": response })
        )?
    );
    Ok(())
}

fn page_contains_message(messages: &[Value], message_id: &str) -> bool {
    messages.iter().any(|message| {
        message
            .get("info")
            .and_then(|info| info.get("id"))
            .and_then(Value::as_str)
            == Some(message_id)
    })
}

fn assistant_response_to<'a>(messages: &'a Value, user_message_id: &str) -> Option<&'a Value> {
    messages
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|message| {
                message
                    .get("info")
                    .and_then(|info| info.get("role"))
                    .and_then(Value::as_str)
                    == Some("assistant")
                    && message
                        .get("info")
                        .and_then(|info| info.get("parentId"))
                        .and_then(Value::as_str)
                        == Some(user_message_id)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronous_run_selects_only_the_submitted_turn_response() {
        let messages = json!({
            "items": [
                { "info": { "role": "assistant", "id": "stale", "parentId": "old-prompt" } },
                { "info": { "role": "assistant", "id": "final", "parentId": "prompt" } },
                { "info": { "role": "user", "id": "prompt" } },
                { "info": { "role": "assistant", "id": "old", "parentId": "older-prompt" } }
            ]
        });
        assert_eq!(assistant_response_to(&messages, "prompt").unwrap()["info"]["id"], "final");
        assert!(assistant_response_to(&messages, "failed-prompt").is_none());
    }
}
