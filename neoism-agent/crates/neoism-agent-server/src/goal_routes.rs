//! Persistent "goal" routes, modeled on Codex's durable goal concept.
//!
//! A goal is a high-level objective the user states once; the agent then keeps
//! it in mind across every turn and (optionally) does web research toward it.
//! The goal is stored on [`SessionInfo`] (in its `extra` map) so it persists to
//! the session store automatically and is injected into the model context each
//! turn by `session_context::provider_messages_for_session`.

use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{event_type, EventPayload, SessionGoal, SessionInfo};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{ensure_session, now_millis};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetGoalRequest {
    /// The goal text. An empty/whitespace string clears the goal.
    pub(crate) text: String,
    /// Optional URLs to scrape via firecrawl and attach as research notes.
    /// Ignored when `FIRECRAWL_API_KEY` is not configured.
    #[serde(default)]
    pub(crate) research_urls: Vec<String>,
    /// Paused goals remain stored but do not force autonomous continuation.
    #[serde(default)]
    pub(crate) paused: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalResponse {
    goal: Option<SessionGoal>,
    research_enabled: bool,
}

/// Return the active goal (or `null`).
pub(crate) async fn session_goal_get(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<GoalResponse>, ApiError> {
    let info = ensure_session(&state, &session_id).await?;
    require_enabled(state.services(), &info)?;
    Ok(Json(goal_response(&info)))
}

/// Set (or clear, when empty) the active goal.
///
/// When `researchUrls` are provided and firecrawl is configured, each URL is
/// scraped and attached to the goal as a research note.
pub(crate) async fn session_goal_set(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: Option<Json<SetGoalRequest>>,
) -> Result<Json<GoalResponse>, ApiError> {
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let mut info = ensure_session(&state, &session_id).await?;
    require_enabled(state.services(), &info)?;

    if request.text.trim().is_empty() {
        info.clear_goal();
        persist(&state, &mut info).await?;
        return Ok(Json(goal_response(&info)));
    }

    let now = now_millis();
    let mut goal = neoism_agent_builtins::plugin::goals::set(
        info.goal(), &request.text, request.paused, now,
    ).expect("non-empty goal text was checked above");

    // Optional firecrawl-backed research. Gated behind the API key: when the
    // key is missing we simply skip research rather than failing the request.
    if !request.research_urls.is_empty() && neoism_agent_builtins::plugin::goals::research_enabled() {
        for url in &request.research_urls {
            match neoism_agent_builtins::plugin::goals::scrape_url(url).await {
                Ok(page) => goal.research.push(
                    neoism_agent_builtins::plugin::goals::research_note(page, now),
                ),
                Err(error) => {
                    tracing::warn!(
                        target: "neoism_agent::goal",
                        url = %url,
                        error = %error,
                        "firecrawl research failed"
                    );
                }
            }
        }
    }

    info.set_goal(&goal);
    persist(&state, &mut info).await?;
    Ok(Json(goal_response(&info)))
}

/// Clear the active goal.
pub(crate) async fn session_goal_clear(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<GoalResponse>, ApiError> {
    let mut info = ensure_session(&state, &session_id).await?;
    require_enabled(state.services(), &info)?;
    info.clear_goal();
    persist(&state, &mut info).await?;
    Ok(Json(goal_response(&info)))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalResearchRequest {
    /// URL to scrape via firecrawl.
    pub(crate) url: String,
}

/// Scrape a URL via firecrawl and attach it
/// to the active goal as a research note.
pub(crate) async fn session_goal_research(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<GoalResearchRequest>,
) -> Result<Json<GoalResponse>, ApiError> {
    let mut info = ensure_session(&state, &session_id).await?;
    require_enabled(state.services(), &info)?;
    if !neoism_agent_builtins::plugin::goals::research_enabled() {
        return Err(ApiError::bad_request(format!(
            "web research is disabled: set {} to enable firecrawl",
            neoism_agent_builtins::plugin::goals::FIRECRAWL_API_KEY_ENV
        )));
    }
    let mut goal = info.goal().ok_or_else(|| {
        ApiError::bad_request("no active goal; set one with /goal first")
    })?;

    let page = neoism_agent_builtins::plugin::goals::scrape_url(&request.url)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let now = now_millis();
    goal = neoism_agent_builtins::plugin::goals::attach_research(goal, page, now);
    info.set_goal(&goal);
    persist(&state, &mut info).await?;
    Ok(Json(goal_response(&info)))
}

fn require_enabled(services: &neoism_agent_service_api::AgentServices, info: &SessionInfo) -> Result<(), ApiError> {
    if crate::plugins::enabled(services, &info.directory, "dev.neoism.goals") {
        Ok(())
    } else {
        Err(ApiError::not_found(
            "Goal plugin is disabled for the workspace",
        ))
    }
}

async fn persist(state: &AppState, info: &mut SessionInfo) -> Result<(), ApiError> {
    let goal_updated = info.goal().map(|goal| goal.updated).unwrap_or(0);
    info.time.updated = now_millis()
        .max(info.time.updated.saturating_add(1))
        .max(goal_updated);
    state.inner.store.update_session(info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": info.id.to_string(), "info": info }),
    ));
    Ok(())
}

fn goal_response(info: &SessionInfo) -> GoalResponse {
    GoalResponse {
        goal: info.goal(),
        research_enabled: neoism_agent_builtins::plugin::goals::research_enabled(),
    }
}
