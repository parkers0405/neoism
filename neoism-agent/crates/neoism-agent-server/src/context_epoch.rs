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
    let observed = observe(state, info);
    let previous = state
        .inner
        .store
        .get_context_epoch(info.id.as_str())
        .await?;
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
                    json!({ "sessionID": info.id, "epoch": epoch }),
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

fn observe(state: &AppState, info: &SessionInfo) -> ContextSnapshot {
    let mut sources = BTreeMap::new();
    sources.insert(
        "config".to_string(),
        json!(crate::config::snapshot(state.services(), &info.directory).ok().map(|snapshot| snapshot.identity)),
    );
    sources.insert(
        "environment".to_string(),
        json!({
            "directory": info.directory,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        }),
    );
    sources.insert(
        "instructions".to_string(),
        json!(crate::instruction::system(state.services(), &info.directory)),
    );
    let memory_enabled = state
        .services()
        .memory
        .as_ref()
        .is_some_and(|memory| crate::mcp::builtin_enabled(&info.directory, memory.id(), state));
    if memory_enabled {
        for fragment in state.services().context_fragments(std::path::Path::new(&info.directory)) {
            sources.insert(format!("service:{}", fragment.id), json!(fragment.content));
        }
    }
    sources.insert(
        "calendar".to_string(),
        json!({ "unixDay": crate::now_millis() / 86_400_000 }),
    );
    ContextSnapshot { sources }
}
