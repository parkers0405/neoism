use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use neoism_agent_core::{event_type, EventPayload, QuestionRequestInfo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;
use crate::{permission_grants, permission_request_allowed};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PermissionReplyRequest {
    pub(crate) reply: Option<String>,
    pub(crate) response: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct QuestionReplyRequest {
    pub(crate) answers: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InteractionListQuery {
    pub(crate) session_id: Option<String>,
}

pub(crate) async fn permission_list(
    State(state): State<AppState>,
    Query(query): Query<InteractionListQuery>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Json<Vec<Value>> {
    let requests = state
        .inner
        .permissions
        .read()
        .await
        .values()
        .filter(|request| {
            query
                .session_id
                .as_ref()
                .is_none_or(|id| &request.session_id == id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let allowed_sessions = allowed_sessions(
        &state,
        claims.as_ref().map(|Extension(claims)| claims),
        requests.iter().map(|request| request.session_id.as_str()),
    )
    .await;
    let mut enriched = Vec::with_capacity(requests.len());
    for request in requests {
        if allowed_sessions
            .as_ref()
            .is_some_and(|ids| !ids.contains(&request.session_id))
        {
            continue;
        }
        enriched.push(
            crate::permission_runtime::permission_request_payload(&state, &request).await,
        );
    }
    Json(enriched)
}

pub(crate) async fn permission_reply(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(reply): Json<PermissionReplyRequest>,
) -> Result<Json<bool>, ApiError> {
    let reply_kind = reply
        .reply
        .clone()
        .or_else(|| reply.response.clone())
        .unwrap_or_else(|| "once".to_string());
    let pending = state
        .inner
        .permission_waiters
        .write()
        .await
        .remove(&request_id);
    let removed = state.inner.permissions.write().await.remove(&request_id);
    let durable_resolved = state
        .inner
        .store
        .resolve_interaction(&request_id, &reply_kind, json!(reply))
        .await?;
    if !durable_resolved && pending.is_none() && removed.is_none() {
        return Ok(Json(true));
    }
    let mut publish = Vec::new();
    let mut sends = Vec::new();

    if let Some(pending) = pending {
        if pending.sender.is_closed()
            || pending
                .cancel
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            state.publish(EventPayload::new(event_type::PERMISSION_REPLIED,
                json!({"sessionID": pending.request.session_id, "requestID": request_id, "reply": "reject"})));
            return Ok(Json(true));
        }
        publish.push((pending.request.clone(), reply_kind.clone()));
        match reply_kind.as_str() {
            "reject" => {
                sends.push((
                    pending.sender,
                    Err(reply
                        .message
                        .clone()
                        .unwrap_or_else(|| "Permission rejected".to_string())),
                ));
                let same_session = {
                    let waiters = state.inner.permission_waiters.read().await;
                    waiters
                        .iter()
                        .filter(|(_, item)| {
                            item.request.session_id == pending.request.session_id
                        })
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>()
                };
                for id in same_session {
                    if let Some(item) =
                        state.inner.permission_waiters.write().await.remove(&id)
                    {
                        state.inner.permissions.write().await.remove(&id);
                        publish.push((item.request.clone(), "reject".to_string()));
                        sends.push((item.sender, Err("Permission rejected".to_string())));
                    }
                }
            }
            "always" => {
                let grants = permission_grants(&pending.request, true);
                let (tenant_id, project_id) = state
                    .inner
                    .store
                    .get_session(&pending.request.session_id)
                    .await?
                    .map(|session| {
                        (
                            crate::caller::session_tenant(&session).to_string(),
                            session.project_id,
                        )
                    })
                    .unwrap_or_else(|| ("local".to_string(), "global".to_string()));
                let approval_scope = (tenant_id.clone(), project_id.clone());
                let approvals = {
                    let mut approval_map = state.inner.permission_approvals.write().await;
                    if pending.sender.is_closed()
                        || pending.cancel.as_ref().is_some_and(|flag| {
                            flag.load(std::sync::atomic::Ordering::SeqCst)
                        })
                    {
                        return Ok(Json(true));
                    }
                    let approvals = approval_map.entry(approval_scope).or_default();
                    approvals.extend(grants.clone());
                    approvals.clone()
                };
                state
                    .inner
                    .store
                    .save_permission_approvals(&tenant_id, &project_id, &approvals)
                    .await?;
                sends.push((pending.sender, Ok(grants)));
                let same_session = {
                    let waiters = state.inner.permission_waiters.read().await;
                    waiters
                        .iter()
                        .filter(|(_, item)| {
                            item.request.session_id == pending.request.session_id
                        })
                        .filter(|(_, item)| {
                            permission_request_allowed(&item.request, &approvals)
                        })
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>()
                };
                for id in same_session {
                    if let Some(item) =
                        state.inner.permission_waiters.write().await.remove(&id)
                    {
                        state.inner.permissions.write().await.remove(&id);
                        publish.push((item.request.clone(), "always".to_string()));
                        sends.push((item.sender, Ok(Vec::new())));
                    }
                }
            }
            _ => {
                sends.push((
                    pending.sender,
                    Ok(permission_grants(&pending.request, false)),
                ));
            }
        }
    }

    let published_specific = !publish.is_empty();
    for (request, reply_kind) in publish {
        let mut payload =
            crate::permission_runtime::permission_request_payload(&state, &request).await;
        payload["requestID"] = json!(request.id);
        payload["reply"] = json!(reply_kind);
        state.publish(EventPayload::new(event_type::PERMISSION_REPLIED, payload));
    }
    if !published_specific {
        if let Some(request) = removed.as_ref() {
            let mut payload =
                crate::permission_runtime::permission_request_payload(&state, request)
                    .await;
            payload["requestID"] = json!(request_id);
            payload["reply"] = json!(reply);
            state.publish(EventPayload::new(event_type::PERMISSION_REPLIED, payload));
        } else {
            state.publish(EventPayload::new(
                event_type::PERMISSION_REPLIED,
                json!({ "requestID": request_id, "reply": reply }),
            ));
        }
    }
    for (sender, result) in sends {
        let _ = sender.send(result);
    }
    Ok(Json(true))
}

pub(crate) async fn question_list(
    State(state): State<AppState>,
    Query(query): Query<InteractionListQuery>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Json<Vec<QuestionRequestInfo>> {
    let mut requests = state
        .inner
        .questions
        .read()
        .await
        .values()
        .filter(|request| {
            query
                .session_id
                .as_ref()
                .is_none_or(|id| &request.session_id == id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let allowed_sessions = allowed_sessions(
        &state,
        claims.as_ref().map(|Extension(claims)| claims),
        requests.iter().map(|request| request.session_id.as_str()),
    )
    .await;
    if let Some(ids) = allowed_sessions {
        requests.retain(|request| ids.contains(&request.session_id));
    }
    Json(requests)
}

async fn allowed_sessions<'a>(
    state: &AppState,
    claims: Option<&crate::caller::CallerClaims>,
    session_ids: impl IntoIterator<Item = &'a str>,
) -> Option<std::collections::HashSet<String>> {
    let claims = claims?;
    // Only canonical sessions represented in the pending snapshot are relevant.
    // In particular, an empty snapshot must not read any session history.
    let session_ids = session_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let mut allowed = std::collections::HashSet::new();
    for session_id in session_ids {
        match state.inner.store.get_session(session_id).await {
            Ok(Some(session)) if crate::caller::allows_session(claims, &session) => {
                allowed.insert(session_id.to_string());
            }
            Err(error) => {
                tracing::warn!(session_id, %error, "failed to authorize pending interaction session");
            }
            _ => {}
        }
    }
    // Only absent claims mean unfiltered local access. Missing or unreadable
    // sessions with claims must remain excluded, never fall back to local access.
    Some(allowed)
}

pub(crate) async fn question_reply(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(reply): Json<QuestionReplyRequest>,
) -> Json<bool> {
    let removed = state.inner.questions.write().await.remove(&request_id);
    let pending = state
        .inner
        .question_waiters
        .write()
        .await
        .remove(&request_id);
    let durable_resolved = match state
        .inner
        .store
        .resolve_interaction(&request_id, "answered", json!(reply))
        .await
    {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::warn!(request_id, %error, "failed to persist question resolution");
            false
        }
    };
    if !durable_resolved && pending.is_none() && removed.is_none() {
        return Json(true);
    }
    if let Some(pending) = pending {
        let _ = pending.sender.send(Ok(reply.answers.clone()));
    }
    state.publish(EventPayload::new(
        event_type::QUESTION_REPLIED,
        json!({ "requestID": request_id, "reply": reply, "info": removed }),
    ));
    Json(true)
}

pub(crate) async fn question_reject(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
) -> Json<bool> {
    let removed = state.inner.questions.write().await.remove(&request_id);
    let pending = state
        .inner
        .question_waiters
        .write()
        .await
        .remove(&request_id);
    let durable_resolved = match state
        .inner
        .store
        .resolve_interaction(&request_id, "rejected", json!({ "rejected": true }))
        .await
    {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::warn!(request_id, %error, "failed to persist question rejection");
            false
        }
    };
    if !durable_resolved && pending.is_none() && removed.is_none() {
        return Json(true);
    }
    if let Some(pending) = pending {
        let _ = pending
            .sender
            .send(Err("question request was rejected".to_string()));
    }
    state.publish(EventPayload::new(
        event_type::QUESTION_REJECTED,
        json!({ "requestID": request_id, "info": removed }),
    ));
    Json(true)
}

pub(crate) async fn cancel_session_interactions(state: &AppState, session_id: &str) {
    if let Err(error) = state
        .inner
        .store
        .cancel_session_interactions(session_id)
        .await
    {
        tracing::warn!(session_id, %error, "failed to persist interaction cancellation");
    }

    let permission_ids = state
        .inner
        .permissions
        .read()
        .await
        .iter()
        .filter(|(_, request)| request.session_id == session_id)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in permission_ids {
        let request = state.inner.permissions.write().await.remove(&id);
        if let Some(waiter) = state.inner.permission_waiters.write().await.remove(&id) {
            let _ = waiter.sender.send(Err("Session cancelled".to_string()));
        }
        state.publish(EventPayload::new(
            event_type::PERMISSION_REPLIED,
            json!({ "requestID": id, "reply": "cancelled", "info": request }),
        ));
    }

    let question_ids = state
        .inner
        .questions
        .read()
        .await
        .iter()
        .filter(|(_, request)| request.session_id == session_id)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in question_ids {
        let request = state.inner.questions.write().await.remove(&id);
        if let Some(waiter) = state.inner.question_waiters.write().await.remove(&id) {
            let _ = waiter.sender.send(Err("Session cancelled".to_string()));
        }
        state.publish(EventPayload::new(
            event_type::QUESTION_REJECTED,
            json!({ "requestID": id, "reason": "cancelled", "info": request }),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{PermissionRequestInfo, SessionInfo};

    struct Fixture {
        state: AppState,
        root: std::path::PathBuf,
    }

    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "neoism-interaction-list-{}",
                neoism_agent_core::new_session_id()
            ));
            let state = AppState::open_database(root.join("agent.db")).await.unwrap();
            Self { state, root }
        }

        async fn invalid_history(&self) {
            // Use a separate public connection, not SessionStore's private DB.
            // Valid JSON missing required SessionInfo fields breaks canonical
            // deserialization without corrupting unrelated targeted reads.
            let path = self.root.join("agent.db");
            let database = turso::Builder::new_local(path.to_str().unwrap())
                .experimental_index_method(true)
                .build()
                .await
                .unwrap();
            let connection = database.connect().unwrap();
            connection
                .execute(
                    "INSERT INTO sessions (id, tenant_id, info_json, updated) VALUES ('invalid', 'tenant-b', '{}', 1)",
                    Vec::<turso::Value>::new(),
                )
                .await
                .unwrap();
            assert!(self.state.inner.store.get_session("invalid").await.is_err());
        }

        async fn finish(self) {
            self.state.shutdown().await.unwrap();
            drop(self.state);
            let _ = std::fs::remove_dir_all(self.root);
        }
    }

    fn claims() -> crate::caller::CallerClaims {
        crate::caller::CallerClaims {
            subject: "actor-a".into(),
            workspace_id: Some("workspace-a".into()),
            tenant_id: "tenant-a".into(),
            directory_prefixes: Vec::new(),
            hosted: true,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
            resolved: None,
            worker: None,
        }
    }

    async fn add_session(state: &AppState, id: &str, tenant: &str, workspace: &str) {
        let session: SessionInfo = serde_json::from_value(json!({
            "id": id, "slug": id, "projectId": "global", "directory": "/tmp",
            "title": id, "version": "test", "time": {"created": 1, "updated": 1},
            "workspaceId": workspace, "neoismTenantId": tenant,
        }))
        .unwrap();
        state.inner.store.insert_session(&session).await.unwrap();
    }

    async fn add_pending(state: &AppState, id: &str, session_id: &str) {
        state.inner.permissions.write().await.insert(
            id.into(),
            PermissionRequestInfo {
                id: id.into(),
                session_id: session_id.into(),
                message_id: "message".into(),
                title: "Allow read".into(),
                permission: "read".into(),
                patterns: vec!["file.txt".into()],
                always: Vec::new(),
                tool: None,
                metadata: None,
            },
        );
        state.inner.questions.write().await.insert(
            id.into(),
            QuestionRequestInfo {
                id: id.into(),
                session_id: session_id.into(),
                message_id: "message".into(),
                questions: vec![json!({"label": "Proceed?"})],
            },
        );
    }

    async fn assert_lists(
        state: &AppState,
        session_id: Option<&str>,
        claims: Option<crate::caller::CallerClaims>,
        expected: &[&str],
    ) {
        let query = InteractionListQuery {
            session_id: session_id.map(str::to_owned),
        };
        let permissions = permission_list(
            State(state.clone()),
            Query(query.clone()),
            claims.clone().map(Extension),
        )
        .await
        .0;
        let questions = question_list(State(state.clone()), Query(query), claims.map(Extension))
            .await
            .0;
        let mut permission_ids = permissions
            .iter()
            .map(|value| value["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        let mut question_ids = questions.iter().map(|request| request.id.as_str()).collect::<Vec<_>>();
        let mut expected = expected.to_vec();
        permission_ids.sort_unstable();
        question_ids.sort_unstable();
        expected.sort_unstable();
        assert_eq!(permission_ids, expected);
        assert_eq!(question_ids, expected);
    }

    #[tokio::test]
    async fn empty_lists_and_filtered_empty_lists_ignore_invalid_history() {
        let fixture = Fixture::new().await;
        fixture.invalid_history().await;
        assert_lists(&fixture.state, None, Some(claims()), &[]).await;
        add_pending(&fixture.state, "invalid-request", "invalid").await;
        assert_lists(&fixture.state, Some("no-pending"), Some(claims()), &[]).await;
        fixture.finish().await;
    }

    #[tokio::test]
    async fn pending_lists_authorize_canonical_sessions_and_isolate_invalid_history() {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        add_session(state, "allowed", "tenant-a", "workspace-a").await;
        add_session(state, "other-tenant", "tenant-b", "workspace-a").await;
        add_session(state, "other-workspace", "tenant-a", "workspace-b").await;
        fixture.invalid_history().await;
        add_pending(state, "allowed-1", "allowed").await;
        add_pending(state, "allowed-2", "allowed").await;
        add_pending(state, "denied-tenant", "other-tenant").await;
        add_pending(state, "denied-workspace", "other-workspace").await;
        add_pending(state, "unknown", "missing").await;
        // Unrelated invalid history must not turn tenant filtering off.
        assert_lists(state, None, Some(claims()), &["allowed-1", "allowed-2"]).await;
        // A represented session that fails deserialization must fail closed too.
        add_pending(state, "failed", "invalid").await;
        assert_lists(state, None, Some(claims()), &["allowed-1", "allowed-2"]).await;
        assert_lists(state, Some("allowed"), Some(claims()), &["allowed-1", "allowed-2"]).await;
        assert_lists(state, Some("invalid"), Some(claims()), &[]).await;
        // No claims preserves local access, even for unknown/failed sessions.
        assert_lists(
            state,
            None,
            None,
            &["allowed-1", "allowed-2", "denied-tenant", "denied-workspace", "unknown", "failed"],
        )
        .await;
        assert_lists(state, Some("missing"), None, &["unknown"]).await;
        fixture.finish().await;
    }

    #[tokio::test]
    async fn cancellation_still_clears_both_pending_maps() {
        let fixture = Fixture::new().await;
        add_pending(&fixture.state, "cancelled", "session").await;
        add_pending(&fixture.state, "unrelated", "other").await;
        cancel_session_interactions(&fixture.state, "session").await;
        assert_lists(&fixture.state, None, None, &["unrelated"]).await;
        fixture.finish().await;
    }
}
