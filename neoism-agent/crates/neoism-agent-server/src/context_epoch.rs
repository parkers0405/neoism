use std::collections::BTreeMap;

use neoism_agent_core::{event_type, EventPayload, SessionInfo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextSnapshot {
    pub(crate) sources: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextEpoch {
    pub(crate) baseline: ContextSnapshot,
    pub(crate) snapshot: ContextSnapshot,
    pub(crate) generation: u64,
    pub(crate) baseline_seq: u64,
    pub(crate) updated: u64,
}

pub(crate) async fn reconcile(
    state: &AppState,
    info: &mut SessionInfo,
) -> Result<ContextEpoch, ApiError> {
    let _guard = state
        .inner
        .session_coordinator
        .context_reconciliation_guard(info.id.as_str())
        .await;
    let previous = state
        .inner
        .store
        .get_context_epoch(info.id.as_str())
        .await?;
    let observed = observe(state, info, previous.as_ref()).await?;
    let (epoch, changed) = match previous {
        None => (
            ContextEpoch {
                baseline: observed.clone(),
                snapshot: observed,
                generation: 1,
                baseline_seq: state
                    .inner
                    .store
                    .message_sequence(info.id.as_str())
                    .await?,
                updated: crate::now_millis(),
            },
            true,
        ),
        Some(mut epoch) if epoch.snapshot != observed => {
            epoch.snapshot = observed;
            epoch.generation = epoch.generation.saturating_add(1);
            epoch.updated = crate::now_millis();
            (epoch, true)
        }
        Some(epoch) => (epoch, false),
    };
    if changed {
        state
            .put_context_epoch_with_event(
                info.id.as_str(),
                &epoch,
                EventPayload::new(
                    event_type::SESSION_CONTEXT_UPDATED,
                    json!({ "sessionID": info.id, "epoch": public_epoch(&epoch) }),
                ),
            )
            .await?;
    }
    info.extra.insert(
        "contextEpoch".to_string(),
        serde_json::to_value(&epoch)
            .map_err(|error| ApiError::internal(error.to_string()))?,
    );
    Ok(epoch)
}

pub(crate) fn from_session(info: &SessionInfo) -> Option<ContextEpoch> {
    serde_json::from_value(info.extra.get("contextEpoch")?.clone()).ok()
}

pub(crate) fn instruction_updates(info: &SessionInfo) -> Vec<(Option<String>, String)> {
    from_session(info)
        .and_then(|epoch| epoch.snapshot.sources.get("instructionUpdates").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| {
            Some((
                value.get("afterMessageId").and_then(Value::as_str).map(str::to_owned),
                value.get("content")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// Tool/catalog waits and provider retries can outlive the request assembly.
/// Recheck at the actual dispatch boundary without rebuilding plugin transforms.
pub(crate) async fn refresh_request(
    state: &AppState,
    session_id: &str,
    messages: &mut Vec<neoism_agent_core::ProviderMessage>,
) -> Result<(), ApiError> {
    let mut info = state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    reconcile(state, &mut info).await?;
    for update in crate::message_model::provider_messages_with_updates(
        &[],
        None,
        &instruction_updates(&info),
    ) {
        if !messages.iter().any(|message| message.content == update.content) {
            messages.push(update);
        }
    }
    Ok(())
}

pub(crate) fn public_epoch(epoch: &ContextEpoch) -> Value {
    json!({
        "generation": epoch.generation,
        "baselineSeq": epoch.baseline_seq,
        "updated": epoch.updated,
        "sources": epoch.snapshot.sources.keys()
            .filter(|key| !matches!(key.as_str(), "instructionState" | "instructionUpdates"))
            .collect::<Vec<_>>(),
        "instructionSources": epoch.snapshot.sources.get("instructionState")
            .cloned()
            .and_then(|value| serde_json::from_value::<crate::instruction::InstructionState>(value).ok())
            .map(|state| state.source_names()).unwrap_or_default(),
    })
}

/// Committing a revert starts a fresh ambient instruction baseline. Nested
/// instruction entries are owned by the surviving conversation history.
pub(crate) async fn reset(
    state: &AppState,
    info: &mut SessionInfo,
) -> Result<(), ApiError> {
    let _guard = state
        .inner
        .session_coordinator
        .context_reconciliation_guard(info.id.as_str())
        .await;
    state.inner.store.clear_context_epoch(info.id.as_str()).await?;
    info.extra.remove("contextEpoch");
    Ok(())
}

async fn observe(
    state: &AppState,
    info: &SessionInfo,
    previous: Option<&ContextEpoch>,
) -> Result<ContextSnapshot, ApiError> {
    let mut sources = BTreeMap::new();
    let plugins = state.plugin_snapshot(&info.directory).await;
    sources.insert(
        "config".to_string(),
        json!({ "generation": plugins.generation }),
    );
    sources.insert(
        "environment".to_string(),
        json!({
            "directory": info.directory,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        }),
    );
    let previous_instructions = previous
        .and_then(|epoch| epoch.snapshot.sources.get("instructionState"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let project = state
        .inner
        .workspace_runtimes
        .project_discovery
        .discover(state.services(), info.directory.clone().into())
        .await
        .map_err(|error| {
            ApiError::internal(format!("project discovery failed: {error}"))
        })?;
    let project_root = std::path::Path::new(
        project.info.worktree.as_deref().unwrap_or(&project.directory),
    );
    let instructions = crate::instruction::load(
        state.services(),
        &info.directory,
        project_root,
        plugins.config(),
        plugins.generation,
        &previous_instructions,
    )
    .await;
    let contents = json!(instructions.contents());
    let mut updates = previous
        .and_then(|epoch| epoch.snapshot.sources.get("instructionUpdates"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(previous) = previous.filter(|epoch| {
        epoch.snapshot.sources.get("instructions") != Some(&contents)
    }) {
        let history = state
            .inner
            .store
            .list_context_messages(info.id.as_str())
            .await?;
        let after_message_id = history.last().map(crate::message_id_of);
        let text = instructions.contents().join("\n\n");
        let revision = previous.generation.saturating_add(1);
        let content = if text.is_empty() {
            format!("Ambient instruction update (epoch {revision}): all previous global, ancestor, and configured instructions no longer apply. There are no current ambient instructions.")
        } else {
            format!("Ambient instruction update (epoch {revision}): replace all previous global, ancestor, and configured instructions with the following current instructions. This does not replace independently discovered nested instructions.\n\n{text}")
        };
        updates.push(json!({ "afterMessageId": after_message_id, "content": content }));
    }
    sources.insert("instructions".to_string(), contents);
    sources.insert("instructionState".to_string(), json!(instructions));
    sources.insert("instructionUpdates".to_string(), json!(updates));
    let memory_enabled = state.services().memory.is_some()
        && crate::plugins::enabled(
            &plugins,
            neoism_agent_builtins::plugin::memory_tools::ID,
        );
    if memory_enabled {
        for fragment in state
            .services()
            .context_fragments(std::path::Path::new(&info.directory))
        {
            sources.insert(format!("service:{}", fragment.id), json!(fragment.content));
        }
    }
    sources.insert(
        "calendar".to_string(),
        json!({ "unixDay": crate::now_millis() / 86_400_000 }),
    );
    Ok(ContextSnapshot { sources })
}
