use axum::extract::Path;
use axum::Json;
use neoism_agent_core::{ModelRef, PermissionRule};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) schedule: WorkflowSchedule,
    pub(crate) prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permission: Option<Vec<PermissionRule>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowSchedule {
    pub(crate) frequency: String,
    #[serde(default = "one")]
    pub(crate) interval: u32,
    #[serde(default = "utc")]
    pub(crate) timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) minute: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) time: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) weekdays: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) month_day: Option<u32>,
}

fn one() -> u32 {
    1
}

fn utc() -> String {
    "UTC".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowProjection {
    pub(crate) activation_id: String,
    pub(crate) workflow_id: String,
    pub(crate) workspace_root: String,
    pub(crate) source_path: String,
    pub(crate) source_hash: String,
    pub(crate) definition: WorkflowDefinition,
    pub(crate) active: bool,
    pub(crate) activated_at: u64,
    pub(crate) last_scheduled_at: Option<u64>,
    pub(crate) updated: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowRun {
    pub(crate) id: String,
    pub(crate) activation_id: String,
    pub(crate) workflow_id: String,
    pub(crate) scheduled_at: u64,
    pub(crate) started_at: Option<u64>,
    pub(crate) finished_at: Option<u64>,
    pub(crate) session_id: Option<String>,
    pub(crate) status: String,
    pub(crate) trigger: String,
    pub(crate) error: Option<String>,
    pub(crate) created: u64,
}

pub(crate) fn spawn_scheduler(_state: AppState) {}

pub(crate) async fn workflow_list() -> Json<Value> {
    Json(json!({ "workflows": [], "status": "initializing" }))
}

pub(crate) async fn workflow_get(
    Path(_workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_found("Workflow not found"))
}

pub(crate) async fn workflow_activate(
    Path(_workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_implemented("workflow activation"))
}

pub(crate) async fn workflow_pause(
    Path(_workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_implemented("workflow pause"))
}

pub(crate) async fn workflow_run_now(
    Path(_workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_implemented("workflow execution"))
}

pub(crate) async fn workflow_preview(
    Path(_workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_implemented("workflow preview"))
}

pub(crate) async fn workflow_history(
    Path(_workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_implemented("workflow history"))
}
