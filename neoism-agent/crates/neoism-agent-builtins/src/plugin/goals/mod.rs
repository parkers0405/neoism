use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use neoism_agent_core::{GoalResearchNote, GoalStatus, SessionGoal};
use neoism_agent_plugin_api::{
    AgentPlugin, ContributionMetadata, PluginFuture, PluginHostError, PluginManifest,
    PluginRegistrar, PluginRuntimeError, RouteContribution, RouteDescriptor, RouteHandler,
    PluginScope, RouteMethod, RouteRequest, RouteResponse, RouteScope,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const ID: &str = "dev.neoism.goals";
pub const FIRECRAWL_API_KEY_ENV: &str = "FIRECRAWL_API_KEY";
const MAX_CONTENT_CHARS: usize = 8_000;

/// Persistence and kernel tool dispatch remain server responsibilities.
pub trait GoalsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginRegistrar);
    fn load<'a>(&'a self, session_id: &'a str) -> PluginFuture<'a, Option<SessionGoal>>;
    fn save<'a>(
        &'a self,
        session_id: &'a str,
        goal: Option<SessionGoal>,
    ) -> PluginFuture<'a, Option<SessionGoal>>;
}

pub struct GoalsPlugin { host: Arc<dyn GoalsHost> }

impl GoalsPlugin { pub fn new(host: Arc<dyn GoalsHost>) -> Self { Self { host } } }

impl AgentPlugin for GoalsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(), name: "Goals".into(), version: env!("CARGO_PKG_VERSION").into(),
            internal: true, disableable: true, capabilities: vec!["neoism.goals".into()],
            requires: Vec::new(), event_namespaces: vec!["goal".into()],
            api_prefix: Some(format!("/v2/plugins/{ID}")), config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        for (id, method, suffix, action) in [
            ("v2.plugins.goals.get", RouteMethod::Get, "", GoalRouteAction::Get),
            ("v2.plugins.goals.set", RouteMethod::Post, "", GoalRouteAction::Set),
            ("v2.plugins.goals.clear", RouteMethod::Delete, "", GoalRouteAction::Clear),
            ("v2.plugins.goals.research", RouteMethod::Post, "/research", GoalRouteAction::Research),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor {
                    id: id.into(),
                    method,
                    path: format!("/v2/plugins/{ID}/:session_id{suffix}"),
                    scope: RouteScope::Session,
                    request_schema: None,
                    response_schema: None,
                },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(GoalRoute { host: self.host.clone(), action }),
            });
        }
        self.host.register_tools(registrar);
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum GoalRouteAction {
    Get,
    Set,
    Clear,
    Research,
}

struct GoalRoute {
    host: Arc<dyn GoalsHost>,
    action: GoalRouteAction,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetGoalRequest {
    #[serde(default)]
    text: String,
    #[serde(default)]
    research_urls: Vec<String>,
    #[serde(default)]
    paused: bool,
}

#[derive(Deserialize)]
struct GoalResearchRequest {
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoalResponse {
    goal: Option<SessionGoal>,
    research_enabled: bool,
}

impl RouteHandler for GoalRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let session_id = request
                .session_id
                .as_deref()
                .ok_or_else(|| PluginRuntimeError::new("goal route requires session_id"))?;
            let goal = match self.action {
                GoalRouteAction::Get => self.host.load(session_id).await?,
                GoalRouteAction::Clear => self.host.save(session_id, None).await?,
                GoalRouteAction::Set => {
                    let request: SetGoalRequest = serde_json::from_value(request.body)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    if request.text.trim().is_empty() {
                        self.host.save(session_id, None).await?
                    } else {
                        let now = now_millis();
                        let mut goal = set(
                            self.host.load(session_id).await?,
                            &request.text,
                            request.paused,
                            now,
                        )
                        .expect("non-empty goal was checked");
                        if research_enabled() {
                            for url in request.research_urls {
                                if let Ok(page) = scrape_url(&url).await {
                                    goal.research.push(research_note(page, now));
                                }
                            }
                        }
                        self.host.save(session_id, Some(goal)).await?
                    }
                }
                GoalRouteAction::Research => {
                    if !research_enabled() {
                        return Ok(RouteResponse::json(
                            400,
                            json!({ "message": format!("web research is disabled: set {FIRECRAWL_API_KEY_ENV}") }),
                        ));
                    }
                    let request: GoalResearchRequest = serde_json::from_value(request.body)
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    let Some(goal) = self.host.load(session_id).await? else {
                        return Ok(RouteResponse::json(400, json!({ "message": "no active goal" })));
                    };
                    let page = scrape_url(&request.url)
                        .await
                        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                    self.host
                        .save(session_id, Some(attach_research(goal, page, now_millis())))
                        .await?
                }
            };
            let body = serde_json::to_value(GoalResponse {
                goal,
                research_enabled: research_enabled(),
            })
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(RouteResponse::json(200, body))
        })
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn set(existing: Option<SessionGoal>, text: &str, paused: bool, now: u64) -> Option<SessionGoal> {
    let text = text.trim();
    if text.is_empty() { return None; }
    let mut goal = existing.unwrap_or_default();
    if goal.created == 0 { goal.created = now; }
    if text != goal.text || goal.status != GoalStatus::Active {
        goal.status = GoalStatus::Active;
        goal.summary.clear();
    }
    goal.text = text.to_string();
    goal.paused = paused;
    goal.updated = now.max(goal.updated.saturating_add(1));
    Some(goal)
}

pub fn complete(mut goal: SessionGoal, status: GoalStatus, summary: &str, now: u64) -> SessionGoal {
    goal.status = status;
    if !summary.trim().is_empty() { goal.summary = summary.trim().to_string(); }
    goal.updated = now.max(goal.updated.saturating_add(1));
    goal
}

pub fn attach_research(mut goal: SessionGoal, page: ResearchPage, now: u64) -> SessionGoal {
    goal.research.push(research_note(page, now));
    goal.updated = now.max(goal.updated.saturating_add(1));
    goal
}

pub fn research_note(page: ResearchPage, now: u64) -> GoalResearchNote {
    let content = page.title.filter(|title| !title.trim().is_empty())
        .map(|title| format!("# {title}\n{}", page.markdown)).unwrap_or(page.markdown);
    GoalResearchNote { source: page.url, content, captured: now }
}

#[derive(Clone, Debug)]
pub struct ResearchPage { pub url: String, pub title: Option<String>, pub markdown: String }

pub fn research_enabled() -> bool { firecrawl_api_key().is_some() }

pub async fn scrape_url(url: &str) -> anyhow::Result<ResearchPage> {
    let api_key = firecrawl_api_key().ok_or_else(|| anyhow!("{FIRECRAWL_API_KEY_ENV} is not set; web research is disabled"))?;
    let url = url.trim();
    if url.is_empty() { return Err(anyhow!("cannot scrape an empty URL")); }
    let base = std::env::var("FIRECRAWL_BASE_URL").ok().map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty()).unwrap_or_else(|| "https://api.firecrawl.dev".into());
    let endpoint = format!("{base}/v1/scrape");
    let response = reqwest::Client::builder().timeout(Duration::from_secs(45)).build()?
        .post(&endpoint).bearer_auth(api_key).json(&json!({"url": url, "formats": ["markdown"], "onlyMainContent": true}))
        .send().await.with_context(|| format!("failed to reach firecrawl at {endpoint}"))?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() { return Err(anyhow!("firecrawl scrape failed with HTTP {status}: {}", truncate(&body, 500))); }
    let parsed: ScrapeResponse = serde_json::from_str(&body)?;
    if !parsed.success { return Err(anyhow!("firecrawl reported failure: {}", parsed.error.unwrap_or_else(|| "unknown error".into()))); }
    let data = parsed.data.ok_or_else(|| anyhow!("firecrawl response missing data field"))?;
    let markdown = data.markdown.filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("firecrawl returned no markdown content for {url}"))?;
    Ok(ResearchPage { url: url.into(), title: data.metadata.and_then(|value| value.title), markdown: truncate(&markdown, MAX_CONTENT_CHARS) })
}

fn firecrawl_api_key() -> Option<String> {
    std::env::var(FIRECRAWL_API_KEY_ENV).ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn truncate(text: &str, maximum: usize) -> String {
    if text.chars().count() <= maximum { return text.into(); }
    let mut output = text.chars().take(maximum).collect::<String>();
    output.push_str("\n…(truncated)");
    output
}

#[derive(Deserialize)]
struct ScrapeResponse { #[serde(default)] success: bool, #[serde(default)] data: Option<ScrapeData>, #[serde(default)] error: Option<String> }
#[derive(Deserialize)]
struct ScrapeData { #[serde(default)] markdown: Option<String>, #[serde(default)] metadata: Option<ScrapeMetadata> }
#[derive(Deserialize)]
struct ScrapeMetadata { #[serde(default)] title: Option<String> }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restating_completed_goal_reopens_and_clears_summary() {
        let old = SessionGoal { text: "ship".into(), status: GoalStatus::Complete, summary: "done".into(), updated: 8, ..Default::default() };
        let goal = set(Some(old), "ship", false, 7).unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
        assert!(goal.summary.is_empty());
        assert_eq!(goal.updated, 9);
    }

    #[test]
    fn research_title_is_rendered_into_note() {
        let goal = attach_research(SessionGoal::default(), ResearchPage { url: "u".into(), title: Some("T".into()), markdown: "body".into() }, 2);
        assert_eq!(goal.research[0].content, "# T\nbody");
    }
}