use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, PermissionAction, PermissionRequestInfo,
    PermissionRule,
};
use serde_json::{json, Value};

const PERMISSION_REQUIRED_PREFIX: &str = "NEOISM_PERMISSION_REQUIRED:";

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct PermissionChallenge {
    pub(crate) permission: String,
    pub(crate) target: String,
}

#[derive(Clone, Debug)]
pub(crate) enum PermissionCheckError {
    Required(PermissionChallenge),
    Denied(PermissionChallenge),
}

impl std::fmt::Display for PermissionCheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Required(challenge) => write!(
                formatter,
                "{PERMISSION_REQUIRED_PREFIX}{}",
                serde_json::to_string(challenge).unwrap_or_else(|_| "{}".to_string())
            ),
            Self::Denied(challenge) => write!(
                formatter,
                "tool permission {} for {} is denied",
                challenge.permission, challenge.target
            ),
        }
    }
}

impl std::error::Error for PermissionCheckError {}

impl From<PermissionCheckError> for String {
    fn from(error: PermissionCheckError) -> Self {
        error.to_string()
    }
}

pub(crate) fn permission_required(
    permission: impl Into<String>,
    target: impl Into<String>,
) -> PermissionCheckError {
    PermissionCheckError::Required(PermissionChallenge {
        permission: permission.into(),
        target: target.into(),
    })
}

pub(crate) fn permission_denied(
    permission: impl Into<String>,
    target: impl Into<String>,
) -> PermissionCheckError {
    PermissionCheckError::Denied(PermissionChallenge {
        permission: permission.into(),
        target: target.into(),
    })
}

use crate::interaction::PermissionReplyRequest;
use crate::permission;
use crate::state::{AppState, PermissionPending};

pub(crate) async fn session_permission_respond(
    State(state): State<AppState>,
    Path((_session_id, permission_id)): Path<(String, String)>,
    Json(_reply): Json<PermissionReplyRequest>,
) -> Json<bool> {
    state.inner.permissions.write().await.remove(&permission_id);
    Json(true)
}

pub(crate) fn parse_permission_required_error(error: &str) -> Option<(String, String)> {
    let marker = error.find(PERMISSION_REQUIRED_PREFIX)?;
    let payload = &error[marker + PERMISSION_REQUIRED_PREFIX.len()..];
    let challenge: PermissionChallenge = serde_json::from_str(payload).ok()?;
    Some((challenge.permission, challenge.target))
}

pub(crate) async fn ask_permission_for_tool(
    state: &AppState,
    session_id: &Id,
    message_id: &Id,
    call_id: &str,
    tool_name: &str,
    input: &Value,
    error: &str,
) -> Result<Vec<PermissionRule>, String> {
    let (permission, target) = parse_permission_required_error(error)
        .ok_or_else(|| format!("permission request could not be parsed: {error}"))?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let request = PermissionRequestInfo {
        id: Id::ascending(IdKind::Permission).to_string(),
        session_id: session_id.to_string(),
        message_id: message_id.to_string(),
        title: format!("Allow {tool_name}?"),
        permission,
        patterns: vec![target.clone()],
        always: vec![target],
        tool: Some(json!({ "messageID": message_id, "callID": call_id })),
        metadata: Some(json!({
            "tool": tool_name,
            "input": input,
            "error": error,
        })),
    };
    state.inner.permission_waiters.write().await.insert(
        request.id.clone(),
        PermissionPending {
            request: request.clone(),
            sender,
        },
    );
    state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request.clone());
    state.publish(EventPayload::new(
        event_type::PERMISSION_ASKED,
        permission_request_payload(state, &request).await,
    ));
    receiver
        .await
        .map_err(|_| "permission request was closed".to_string())?
}

pub(crate) async fn permission_request_payload(
    state: &AppState,
    request: &PermissionRequestInfo,
) -> Value {
    let mut payload = json!(request);
    if let Ok(Some(session)) = state.inner.store.get_session(&request.session_id).await {
        payload["sourceSessionID"] = json!(session.id.to_string());
        payload["sourceTitle"] = json!(session.title);
        if let Some(agent) = session.agent.as_ref() {
            payload["sourceAgent"] = json!(agent);
        }
        if let Some(parent_id) = session.parent_id.as_ref() {
            payload["parentSessionID"] = json!(parent_id);
        }
    }
    payload
}

pub(crate) fn permission_grants(
    request: &PermissionRequestInfo,
    always: bool,
) -> Vec<PermissionRule> {
    let patterns = if always && !request.always.is_empty() {
        &request.always
    } else {
        &request.patterns
    };
    patterns
        .iter()
        .map(|pattern| PermissionRule {
            permission: request.permission.clone(),
            pattern: pattern.clone(),
            action: PermissionAction::Allow,
        })
        .collect()
}

pub(crate) fn permission_request_allowed(
    request: &PermissionRequestInfo,
    rules: &[PermissionRule],
) -> bool {
    request.patterns.iter().all(|pattern| {
        permission::evaluate(&request.permission, pattern, rules).action
            == PermissionAction::Allow
    })
}
