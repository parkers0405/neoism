use super::*;
use neoism_protocol::agent::SubagentModel;

pub(crate) fn failure(request: AgentClientMessage, error: String) -> AgentServerMessage {
    match request {
        AgentClientMessage::GetSubagentModels { request_id, provider } => {
            AgentServerMessage::SubagentModels { request_id, provider, models: vec![], reasoning: vec![], error: Some(error) }
        }
        AgentClientMessage::SetSubagentDefault { request_id, provider, .. } => {
            AgentServerMessage::SubagentDefault { request_id, provider, model: None, reasoning: None, error: Some(error) }
        }
        AgentClientMessage::GetSubagentDefault { request_id } => {
            AgentServerMessage::SubagentDefault { request_id, provider: String::new(), model: None, reasoning: None, error: Some(error) }
        }
        _ => unreachable!("not a picker request"),
    }
}

pub(crate) async fn run(
    session: AgentSession,
    request: AgentClientMessage,
    directory: &std::path::Path,
) -> AgentServerMessage {
    let inner = &session.inner;
    let directory = percent_encode(&directory.to_string_lossy());
    let default_path = format!("/v2/subagents/default?directory={directory}");
    let result = match &request {
        AgentClientMessage::GetSubagentModels { request_id, provider } => {
            let models = if provider == "neoism" {
                Ok((vec![], vec![]))
            } else {
                let provider = percent_encode(provider);
                let path = format!("/v2/external/options/preview?provider={provider}&directory={directory}");
                // Provider setup can exceed the normal daemon read timeout.
                picker_get(inner, &path).await.and_then(|value| Ok((model_options(&value)?, selector_options(&value, "thought_level"))))
            };
            models.map(|(models, reasoning)| AgentServerMessage::SubagentModels {
                request_id: request_id.clone(), provider: provider.clone(), models, reasoning, error: None,
            })
        }
        AgentClientMessage::GetSubagentDefault { request_id } => {
            picker_get(inner, &default_path).await.and_then(|value| default_reply(request_id, value))
        }
        AgentClientMessage::SetSubagentDefault { request_id, provider, model, reasoning } => {
            http_post_json(inner, &default_path, &json!({"provider": provider, "model": model, "reasoning": reasoning}))
                .await.and_then(|value| default_reply(request_id, value))
        }
        _ => unreachable!("not a picker request"),
    };
    result.unwrap_or_else(|error| failure(request, error))
}

async fn picker_get(inner: &AgentInner, path: &str) -> Result<Value, String> {
    wait_for_local_agent_server(inner).await?;
    let response = inner.authorize_agent_request(
        inner.agent_http.get(format!("{}{path}", inner.agent_server)).timeout(AGENT_MUTATION_TIMEOUT),
    ).send().await.map_err(|error| error.without_url().to_string())?;
    let status = response.status();
    let body = response.json::<Value>().await;
    if !status.is_success() {
        let message = body.as_ref().ok().and_then(|value| value["message"].as_str());
        return Err(message.map(str::to_owned).unwrap_or_else(|| format!("Agent server returned HTTP {status}")));
    }
    body.map_err(|error| error.without_url().to_string())
}

fn default_reply(request_id: &str, value: Value) -> Result<AgentServerMessage, String> {
    #[derive(serde::Deserialize)]
    struct Selection { provider: String, model: Option<String>, reasoning: Option<String> }
    let Selection { provider, model, reasoning } = serde_json::from_value(value)
        .map_err(|error| format!("Invalid subagent default response: {error}"))?;
    Ok(AgentServerMessage::SubagentDefault { request_id: request_id.into(), provider, model, reasoning, error: None })
}

fn model_options(value: &Value) -> Result<Vec<SubagentModel>, String> {
    if value["catalogStale"] == true {
        return Err("The harness model catalog is stale; retry when the harness is available".into());
    }
    if let Some(error) = value["replayError"].as_str() {
        return Err(error.into());
    }
    if !value["configOptions"].is_array() { return Err("Missing harness options".into()); }
    Ok(selector_options(value, "model"))
}

fn selector_options(value: &Value, category: &str) -> Vec<SubagentModel> {
    let selector = value["configOptions"].as_array().into_iter().flatten().find(|item| {
        item["type"] == "select" && (item["category"] == category || item["id"] == category
            || (category == "thought_level" && (matches!(item["category"].as_str(), Some("thought" | "reasoning"))
                || matches!(item["id"].as_str(), Some("thought" | "reasoning" | "effort" | "reasoningEffort" | "variant")))))
            && item["id"].as_str().is_some_and(|id| !id.is_empty())
    });
    let mut seen = std::collections::HashSet::new();
    selector.into_iter().filter_map(|selector| selector["options"].as_array()).flatten()
        .flat_map(|entry| entry["options"].as_array().map(Vec::as_slice).unwrap_or_else(|| std::slice::from_ref(entry)))
        .filter_map(|entry| {
            let id = entry["value"].as_str().filter(|id| !id.is_empty() && seen.insert(*id))?;
            Some(SubagentModel { id: id.into(), label: entry["name"].as_str().unwrap_or(id).into() })
        }).collect()
}

#[test]
fn subagent_picker_projects_the_provider_model_selector_without_substitution() {
    let mut preview = json!({"configOptions":[
        {"id":"mode","type":"select","category":"mode","options":[{"value":"ask"}]},
        {"id":"models","type":"select","category":"model","options":[
            {"group":"family","options":[{"value":"a","name":"A"},{"value":"b"}]},
            {"value":"b","name":"Duplicate"}
        ]}
    ]});
    assert_eq!(model_options(&preview).unwrap(), vec![
        SubagentModel { id: "a".into(), label: "A".into() },
        SubagentModel { id: "b".into(), label: "b".into() },
    ]);
    preview["catalogStale"] = json!(true);
    assert!(model_options(&preview).is_err());
}