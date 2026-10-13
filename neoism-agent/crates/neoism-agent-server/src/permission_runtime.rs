use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, PermissionAction, PermissionRequestInfo,
    PermissionRule,
};
use serde_json::{json, Value};

const PERMISSION_REQUIRED_PREFIX: &str = "NEOISM_PERMISSION_REQUIRED:";

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct PermissionChallenge {
    pub(crate) permission: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) patterns: Vec<String>,
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
                challenge.permission,
                challenge
                    .target
                    .as_deref()
                    .or_else(|| challenge.patterns.first().map(String::as_str))
                    .unwrap_or("*")
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
        target: Some(target.into()),
        patterns: Vec::new(),
    })
}

pub(crate) fn permission_required_many(
    permission: impl Into<String>,
    patterns: Vec<String>,
) -> PermissionCheckError {
    PermissionCheckError::Required(PermissionChallenge {
        permission: permission.into(),
        target: None,
        patterns,
    })
}

pub(crate) fn permission_denied(
    permission: impl Into<String>,
    target: impl Into<String>,
) -> PermissionCheckError {
    PermissionCheckError::Denied(PermissionChallenge {
        permission: permission.into(),
        target: Some(target.into()),
        patterns: Vec::new(),
    })
}

use crate::permission;
use crate::state::{AppState, PermissionPending};

pub(crate) fn parse_permission_required_error(
    error: &str,
) -> Option<(String, Vec<String>)> {
    let marker = error.find(PERMISSION_REQUIRED_PREFIX)?;
    let payload = &error[marker + PERMISSION_REQUIRED_PREFIX.len()..];
    let mut challenge: PermissionChallenge = serde_json::from_str(payload).ok()?;
    if let Some(target) = challenge.target.take() {
        challenge.patterns.push(target);
    }
    challenge.patterns.dedup();
    (!challenge.patterns.is_empty()).then_some((challenge.permission, challenge.patterns))
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
    ask_permission_for_tool_cancellable(
        state, session_id, message_id, call_id, tool_name, input, error, None,
    )
    .await
}

pub(crate) async fn ask_permission_for_tool_cancellable(
    state: &AppState,
    session_id: &Id,
    message_id: &Id,
    call_id: &str,
    tool_name: &str,
    input: &Value,
    error: &str,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Vec<PermissionRule>, String> {
    let (permission, patterns) = parse_permission_required_error(error)
        .ok_or_else(|| format!("permission request could not be parsed: {error}"))?;
    let clipboard = permission == "computer_clipboard";
    let metadata = if clipboard {
        json!({"tool":tool_name,"warning":"Clipboard may be replaced; clipboard history may retain the text. Previous content is not read or restored. Paste may execute multiline commands in terminals. Actual selected method may not need the clipboard.","inputPayloadOmitted":true})
    } else {
        json!({"tool":tool_name,"input":input,"error":error})
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let request = PermissionRequestInfo {
        id: Id::ascending(IdKind::Permission).to_string(),
        session_id: session_id.to_string(),
        message_id: message_id.to_string(),
        title: if clipboard {
            "Allow clipboard replacement? History may retain text; paste may execute commands.".into()
        } else {
            format!("Allow {tool_name}?")
        },
        permission,
        patterns: patterns.clone(),
        always: patterns,
        tool: Some(json!({ "messageID": message_id, "callID": call_id })),
        metadata: Some(metadata),
    };
    state
        .inner
        .store
        .save_permission_request(&request)
        .await
        .map_err(|error| error.to_string())?;
    state.inner.permission_waiters.write().await.insert(
        request.id.clone(),
        PermissionPending {
            request: request.clone(),
            cancel: cancel.clone(),
            sender,
        },
    );
    state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request.clone());
    // Abort-on-drop leaf supervision may drop this future before its select
    // branch runs. Synchronous best-effort map cleanup leaves no live waiter;
    // pending.cancel independently prevents late approval side effects.
    struct WaiterCleanup<'a> {
        state: &'a AppState,
        id: &'a str,
    }
    impl Drop for WaiterCleanup<'_> {
        fn drop(&mut self) {
            if let Ok(mut waiters) = self.state.inner.permission_waiters.try_write() {
                waiters.remove(self.id);
            }
            if let Ok(mut permissions) = self.state.inner.permissions.try_write() {
                permissions.remove(self.id);
            }
        }
    }
    let _waiter_cleanup = WaiterCleanup {
        state,
        id: &request.id,
    };
    state.publish(EventPayload::new(
        event_type::PERMISSION_ASKED,
        json!(request),
    ));
    if let Some(cancel) = cancel {
        tokio::select! {
            biased;
            _ = crate::session_loop::wait_for_cancellation(cancel) => {
                // Pending entries carry the cancellation flag, so late replies
                // cannot grant permissions even while durable cleanup is pending.
                state.inner.permission_waiters.write().await.remove(&request.id);
                state.inner.permissions.write().await.remove(&request.id);
                // Map cleanup must not depend on durable-store responsiveness.
                let _ = tokio::time::timeout(std::time::Duration::from_millis(100),
                    state.inner.store.resolve_interaction(&request.id, "cancelled",
                        json!({"cancelled": true}))).await;
                state.publish(EventPayload::new(event_type::PERMISSION_REPLIED,
                    json!({"sessionID": session_id, "requestID": request.id, "reply": "reject"})));
                Err("permission request aborted".into())
            }
            result = receiver => result.map_err(|_| "permission request was closed".to_string())?,
        }
    } else {
        receiver
            .await
            .map_err(|_| "permission request was closed".to_string())?
    }
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
