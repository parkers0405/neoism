//! Local desktop prompt-workflow capability. Never delegates to general discovery or operator routes.
use axum::{body::Body, response::{IntoResponse, Response}};
use std::path::PathBuf;

pub(crate) async fn handle(state: &crate::AppState, root: PathBuf, body: Body) -> Response {
    #[cfg(unix)]
    { request(state, root, body).await.into_response() }
    #[cfg(not(unix))]
    { let _ = (state, root, body); crate::error::ApiError::forbidden("Local workflows require Unix").into_response() }
}

#[cfg(unix)]
async fn request(state: &crate::AppState, root: PathBuf, body: Body) -> Result<axum::Json<serde_json::Value>, crate::error::ApiError> {
    use crate::{error::ApiError, server_util::now_millis, workflow::{self, WorkflowProjection}};
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Operation { Activate, History, Pause, Run, Source }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Command { operation: Operation, id: String, revision: String, source: String, origin: Option<String> }
    const TRIGGER: &str = "rune-manual";

    let body = axum::body::to_bytes(body, 8192).await.map_err(|_| ApiError::bad_request("Workflow request exceeds 8 KiB"))?;
    let command: Command = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("Invalid prompt workflow request"))?;
    let workspace = root.to_str().ok_or_else(|| ApiError::bad_request("Invalid workspace root"))?.to_owned();
    let guard = state.inner.workflow_reconcile.lock().await;
    let source = workflow::local_prompt_source(state, root.clone(), &command.id).await?;
    if command.revision != format!("sha256:{}", source.source_hash) || command.source != source.source_path {
        return Err(ApiError::conflict("Workflow source changed. Refresh and inspect the new revision."));
    }
    let id = workflow::activation_id(&workspace, &command.id);
    let existing = state.inner.store.get_workflow(&id).await?;
    let pin = state.inner.store.rune_prompt_pin(&id).await?;
    if matches!(command.operation, Operation::Source) {
        let relative = std::path::Path::new(&source.source_path).strip_prefix(&root)
            .map_err(|_| ApiError::forbidden("Workflow source is outside the workspace"))?;
        return Ok(axum::Json(json!({"path": relative})));
    }
    // An ID can collide with previously adopted installation, linked or operator sources.
    // Never mutate their projection or disclose any of their history.
    if existing.as_ref().is_some_and(|prior| prior.workspace_root != workspace || prior.workflow_id != command.id
        || (prior.source_path != source.source_path && !(pin.is_some() && matches!(command.operation, Operation::Activate)))
        || prior.definition.action.is_some()) {
        return Err(ApiError::conflict("Workflow ID is already bound to another source"));
    }
    if matches!(command.operation, Operation::History) {
        let runs = state.inner.store.list_workflow_runs(&id, 50).await?.into_iter()
            .filter(|run| matches!(run.trigger.as_str(), TRIGGER | workflow::RUNE_SCHEDULED_PROMPT))
            .map(|run| json!({"id": run.id, "status": run.status, "created": run.created, "finishedAt": run.finished_at}))
            .collect::<Vec<_>>();
        return Ok(axum::Json(json!({"runs": runs})));
    }
    if matches!(command.operation, Operation::Activate) {
        if !workflow::rune_schedule_eligible(&source.definition) {
            return Err(ApiError::forbidden("Activate requires an eligible workspace-root prompt schedule"));
        }
        let origin = source.local_origin.as_deref().filter(|origin| Some(*origin) == command.origin.as_deref())
            .ok_or_else(|| ApiError::conflict("Workflow origin changed. Refresh and inspect the source."))?;
        let now = now_millis();
        let projection = WorkflowProjection {
            activation_id: id, workflow_id: command.id, workspace_root: workspace,
            source_path: source.source_path, source_hash: source.source_hash, definition: source.definition,
            active: true, activated_at: now, last_scheduled_at: None, updated: now,
        };
        state.inner.store.pin_rune_prompt(&projection, origin).await?;
        workflow::publish_workflow(state, &projection);
        state.inner.workflow_notify.notify_one();
        return Ok(axum::Json(json!({"active": true, "activationID": projection.activation_id})));
    }
    if existing.as_ref().is_some_and(|prior| prior.source_hash != source.source_hash) {
        return Err(ApiError::conflict("Stored workflow revision differs from the current source"));
    }
    if matches!(command.operation, Operation::Pause) {
        let mut projection = existing.filter(|prior| prior.active).ok_or_else(|| ApiError::conflict("Workflow is not active"))?;
        projection.updated = now_millis();
        projection.active = false;
        state.inner.store.set_workflow_active(&id, false, projection.updated).await?;
        workflow::publish_workflow(state, &projection);
        state.inner.workflow_notify.notify_one();
        return Ok(axum::Json(json!({"active": false})));
    }
    // No transitive skill lookup, custom execution directory, retries or replacement of another run.
    if source.definition.directory.is_some() || source.definition.skill.is_some() || source.definition.retry.max_attempts != 1
        || source.definition.concurrency.mode != workflow::WorkflowConcurrencyMode::Forbid {
        return Err(ApiError::forbidden("Run now is limited to workspace-root prompts without skills or retries, using forbid concurrency"));
    }
    let now = now_millis();
    let projection = existing.unwrap_or_else(|| WorkflowProjection {
        activation_id: id, workflow_id: command.id, workspace_root: workspace,
        source_path: source.source_path, source_hash: source.source_hash, definition: source.definition,
        active: false, activated_at: now, last_scheduled_at: None, updated: now,
    });
    if std::path::Path::new(&projection.workspace_root).canonicalize().ok().as_ref() != Some(&root) {
        return Err(ApiError::conflict("Workspace root changed"));
    }
    if state.inner.store.get_workflow(&projection.activation_id).await?.is_none() {
        state.inner.store.upsert_workflow(&projection).await?;
    }
    let run = workflow::claim_manual_run_with_trigger(state, &projection, TRIGGER).await?;
    drop(guard);
    tokio::spawn(workflow::execute_run(state.clone(), projection, run.clone()));
    Ok(axum::Json(json!({"runID": run.id})))
}