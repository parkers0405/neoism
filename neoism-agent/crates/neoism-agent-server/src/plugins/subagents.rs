use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::{Extension, Json};
use neoism_agent_core::{Id, IdKind, ModelRef, SessionInfo, SubagentDefaultConfig, UserModel};
use neoism_agent_service_api::{ConfigDiscoveryScope, ConfigUpdate, ConfigUpdateRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::Mutex;

use crate::error::ApiError;
use crate::execution_activity::SubtaskAdmissionGuard;
use crate::external_agent::{options, ExternalRuntime};
use crate::plugins;
use crate::session_actions::{create_subtask_session, spawn_background_subtask_prompt};
use crate::state::AppState;
use crate::tool::ToolExecutionResult;
use crate::workspace_runtime::PluginGenerationLease;

/// Generation-owned bookkeeping for subagent sessions. The session records
/// remain durable, while runs, queues and keyed reconciliation locks do not.
#[derive(Default)]
pub(crate) struct SubagentWorkspaceRuntime {
    sessions: Mutex<BTreeSet<String>>,
}

impl SubagentWorkspaceRuntime {
    pub(crate) async fn track(&self, session_id: impl Into<String>) {
        self.sessions.lock().await.insert(session_id.into());
    }

    pub(crate) async fn teardown(&self) {
        self.sessions.lock().await.clear();
    }
}

#[cfg(test)]
fn enabled_in_config(config: &neoism_agent_core::AgentConfigDocument) -> bool {
    config
        .plugins
        .get(neoism_agent_builtins::plugin::subagents::ID)
        .is_none_or(|plugin| plugin.enabled)
}

async fn require_enabled(state: &AppState, directory: &str) -> Result<(), String> {
    let snapshot = state.plugin_snapshot(directory).await;
    plugins::enabled(&snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "plugin {} is disabled for this workspace",
                neoism_agent_builtins::plugin::subagents::ID
            )
        })
}

#[derive(Deserialize)]
pub(crate) struct DefaultDirectory {
    directory: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefaultSelection {
    provider: String,
    model: Option<String>,
    reasoning: Option<String>,
}

fn default_directory(
    state: &AppState,
    query: DefaultDirectory,
    headers: &HeaderMap,
    claims: Option<&Extension<crate::caller::CallerClaims>>,
) -> Result<String, ApiError> {
    if !std::path::Path::new(&query.directory).is_absolute() {
        return Err(ApiError::bad_request("An absolute workspace directory is required"));
    }
    let directory = options::admitted_request_directory(
        state,
        claims.map(|Extension(claims)| claims),
        Some(query.directory),
        headers,
    )?;
    guard_config_paths(std::path::Path::new(&directory))?;
    Ok(directory)
}

/// The config service can migrate a legacy project document during a read and
/// writes a sibling temp file. Do not follow symlinks in any of those paths.
fn guard_config_paths(root: &std::path::Path) -> Result<(), ApiError> {
    for (relative, directory) in [
        (".neoism", true), (".neoism/config.json", false),
        (".neoism/mcp.json", false), ("neoism.json", false),
        (".agent", true), (".agent/agent.json", false),
    ] {
        match std::fs::symlink_metadata(root.join(relative)) {
            Ok(metadata) if if directory { metadata.is_dir() } else { metadata.is_file() } => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            _ => return Err(ApiError::forbidden("Unsafe workspace configuration path")),
        }
    }
    Ok(())
}

fn provider_id(provider: &str) -> Result<&'static str, ApiError> {
    match provider.trim() {
        "" | "neoism" => Ok("neoism"),
        provider => ExternalRuntime::resolve(provider)
            .map(ExternalRuntime::provider_id)
            .ok_or_else(|| ApiError::bad_request("Unknown subagent harness")),
    }
}

fn native_model_ref(model: &str) -> Option<ModelRef> {
    crate::model_ref_from_config(model).filter(|model| {
        [&model.provider_id, &model.id]
            .into_iter()
            .all(|id| !id.is_empty() && id.trim() == id)
    })
}

async fn validate_native_model(
    state: &AppState,
    directory: &str,
    model: &str,
) -> Result<(), ApiError> {
    let model_ref = native_model_ref(model).ok_or_else(|| {
        ApiError::bad_request("A Neoism subagent model must use provider/model syntax")
    })?;
    let snapshot = state.plugin_snapshot(directory).await;
    let provider = snapshot
        .provider_services_by_priority()
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::bad_request("No model provider is registered for this workspace"))?;
    let descriptor = provider.descriptor();
    if descriptor.id == model_ref.provider_id && descriptor.models.contains(&model_ref.id) {
        return Ok(());
    }
    let user_model = crate::user_model_from_model_ref(&model_ref);
    let metadata = provider
        .model_metadata(&user_model)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if metadata.api.is_none() {
        return Err(ApiError::bad_request(format!(
            "Unknown Neoism subagent model {model:?}"
        )));
    }
    Ok(())
}

pub(crate) async fn get_default(
    State(state): State<AppState>,
    Query(query): Query<DefaultDirectory>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<DefaultSelection>, ApiError> {
    let directory = default_directory(&state, query, &headers, claims.as_ref())?;
    let config = crate::config::load(state.services(), &directory)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .info;
    let provider = provider_id(config.default_subagent_harness.as_deref().unwrap_or("neoism"))?;
    let (model, reasoning) = config
        .default_subagent
        .map(|selection| (selection.model, selection.reasoning))
        .unwrap_or_default();
    Ok(Json(DefaultSelection {
        provider: provider.into(),
        model,
        reasoning,
    }))
}

pub(crate) async fn set_default(
    State(state): State<AppState>,
    Query(query): Query<DefaultDirectory>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(mut selection): Json<DefaultSelection>,
) -> Result<Json<DefaultSelection>, ApiError> {
    let directory = default_directory(&state, query, &headers, claims.as_ref())?;
    if std::env::var("NEOISM_DISABLE_PROJECT_CONFIG").as_deref() == Ok("1") {
        return Err(ApiError::forbidden("Workspace configuration is disabled"));
    }
    selection.provider = provider_id(&selection.provider)?.into();
    if selection.reasoning.as_deref().is_some_and(|value| value.trim().is_empty()) {
        return Err(ApiError::bad_request("A reasoning value must not be empty"));
    }
    let (option_id, reasoning_option_id) = if let Some(runtime) = ExternalRuntime::from_provider_id(&selection.provider) {
        options::validate_subagent_selection(
            &state, runtime, &directory, selection.model.as_deref(), selection.reasoning.as_deref(),
        ).await?
    } else {
        if let Some(model) = &selection.model {
            validate_native_model(&state, &directory, model).await?;
        }
        (None, None)
    };
    // Serialize this read/modify/write path, and repeat the path check after
    // async provider validation. Never fall back to an installation target.
    static SAVE: Mutex<()> = Mutex::const_new(());
    let _save = SAVE.lock().await;
    guard_config_paths(std::path::Path::new(&directory))?;
    let snapshot = crate::config::snapshot(state.services(), &directory)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let source_id = snapshot.writable_target.source_id;
    if snapshot.workspace != std::path::Path::new(&directory)
        || !snapshot.layers.iter().any(|layer| {
            layer.source_id == source_id && layer.scope == ConfigDiscoveryScope::Workspace && layer.writable
        })
    {
        return Err(ApiError::forbidden("No writable configuration for this workspace"));
    }
    state.services().config.update(&ConfigUpdateRequest {
        workspace: snapshot.workspace,
        source_id,
        update: ConfigUpdate::SetValue {
            path: vec!["defaultSubagent".into()],
            value: json!(SubagentDefaultConfig {
                provider: selection.provider.clone(), model: selection.model.clone(), option_id,
                reasoning: selection.reasoning.clone(), reasoning_option_id,
            }),
        },
    }).await.map_err(|error| ApiError::internal(error.to_string()))?;
    state.publish_config_mutation(&directory).await.map_err(ApiError::internal)?;
    Ok(Json(selection))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubagentTaskInfo {
    id: String,
    session_id: String,
    child_session_id: String,
    agent: String,
    status: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    nested: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopSubagentsRequest {
    task_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopSubagentsResult {
    stopped: Vec<String>,
    cleared_prompts: usize,
}

pub(crate) fn task_agent_name(configured: Option<&str>, input: &Value) -> Result<String, String> {
    if let Some(name) = string_arg(input, "subagent_type").filter(|name| !name.trim().is_empty() && name != "default") {
        return Ok(name);
    }
    if string_arg(input, "task_id").is_some_and(|id| !id.trim().is_empty()) {
        return Err("Continuing a task requires its original subagent_type".into());
    }
    match configured.map(str::trim) {
        None | Some("" | "neoism") => Ok("general".into()),
        Some(name) => crate::external_agent::ExternalRuntime::resolve(name)
            .map(|runtime| runtime.agent_name().to_owned())
            .ok_or_else(|| format!("Unknown agent.defaultSubagentHarness {name:?}; select a supported external harness or 'neoism'")),
    }
}

#[cfg(test)]
#[test]
fn subagent_harness_default_only_applies_to_new_unspecified_tasks() {
    use serde_json::json;
    assert_eq!(task_agent_name(None, &json!({})).unwrap(), "general");
    assert_eq!(task_agent_name(Some("claude-code"), &json!({})).unwrap(), "claude");
    assert_eq!(task_agent_name(Some("claude"), &json!({"subagent_type":"explore"})).unwrap(), "explore");
    assert_eq!(task_agent_name(Some("claude"), &json!({"subagent_type":"codex","task_id":"child"})).unwrap(), "codex");
    assert!(task_agent_name(Some("claude"), &json!({"subagent_type":"default","task_id":"child"})).is_err());
    assert!(task_agent_name(Some("typo"), &json!({})).is_err());
}

pub(crate) async fn start_task_tool(
    state: &AppState,
    snapshot: PluginGenerationLease,
    session_id: &Id,
    input: Value,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ToolExecutionResult, String> {
    let agent_name = task_agent_name(snapshot.config().default_subagent_harness.as_deref(), &input)?;
    let prompt = string_arg(&input, "prompt")
        .ok_or_else(|| "tool argument prompt is required".to_string())?;
    let description = string_arg(&input, "description")
        .unwrap_or_else(|| prompt.chars().take(48).collect::<String>());
    let command = string_arg(&input, "command").unwrap_or_else(|| description.clone());
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let parent = parent_session(state, session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    plugins::enabled(&snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "plugin {} is disabled for this workspace",
                neoism_agent_builtins::plugin::subagents::ID
            )
        })?;
    // Tool clients may serialize the optional continuation ID as an empty string.
    let task_id = string_arg(&input, "task_id").filter(|id| !id.trim().is_empty());
    let continuing = task_id.is_some();
    if !continuing {
        let depth = crate::tool_runtime::session_subtask_depth(state, &parent).await;
        if depth + 1 > crate::tool_runtime::MAX_SUBTASK_DEPTH {
            return Err(format!(
                "subagent depth limit reached ({}): this session is already {depth} level(s) deep in the subagent tree. Do the remaining work directly in this session instead of spawning further subagents.",
                crate::tool_runtime::MAX_SUBTASK_DEPTH
            ));
        }
    }
    if crate::external_agent::is_external_agent(&agent_name) {
        if !crate::caller::native_execution_allowed(
            &crate::caller::session_execution_policy(state.services(), &parent),
        ) {
            return Err(
                "external native subagents are unavailable for this session".into()
            );
        }
        let mut external_options = input.get("external_options").map(|value| {
            serde_json::from_value::<std::collections::BTreeMap<String, String>>(value.clone())
                .map_err(|_| "external_options must be a string-map".to_string())
        }).transpose()?;
        if !continuing && uses_configured_harness(&input) {
            if let Some(saved) = snapshot.config().default_subagent.as_ref().filter(|saved| saved.provider == agent_name) {
                apply_saved_external_options(saved, &mut external_options)?;
            }
        }
        return crate::external_agent::execute_external_task_with_options(
            state,
            &parent,
            &agent_name,
            &command,
            &description,
            prompt,
            task_id,
            background,
            cancel,
            external_options,
        )
        .await;
    }
    if input.get("external_options").is_some() {
        return Err("external_options requires an external subagent_type".into());
    }
    let agents = plugins::agent_catalog(&snapshot, &parent.directory)
        .map_err(|error| error.to_string())?;
    let agent = agents.get(&agent_name).ok_or_else(|| {
        let available = agents
            .list()
            .into_iter()
            .filter(|agent| agent.mode == "subagent")
            .map(|agent| agent.name)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Unknown agent type: {agent_name} is not a valid agent type. Available subagents: {available}"
        )
    })?;
    let child_model = native_task_model(
        snapshot.config().default_subagent.as_ref(), &input, continuing,
        agent.model.as_ref().or(parent.model.as_ref()),
    )?;
    let (child_session_id, child_model) = if let Some(task_id) = task_id.as_deref() {
        if let Some(child) = state
            .inner
            .store
            .get_session(task_id)
            .await
            .map_err(|error| error.to_string())?
        {
            crate::tool_runtime::ensure_child_task_belongs_to_parent(
                state, &parent, &child,
            )
            .await?;
            if child.extra.contains_key("externalAgent") {
                return Err(format!(
                    "task_id {task_id} belongs to an external agent; continue it with its original external subagent_type, not {agent_name}"
                ));
            }
            // Do not reselect a workspace default on continuation. Keep the
            // existing model when continuing the same role.
            let model = if child.agent.as_deref() == Some(agent.name.as_str()) {
                child.model.as_ref().map(crate::user_model_from_model_ref).or(child_model)
            } else {
                child_model
            };
            (child.id.to_string(), model)
        } else {
            let id = create_child(
                state,
                &snapshot,
                &parent,
                &command,
                &description,
                &agent.name,
                child_model.clone(),
            )
            .await?;
            (id, child_model)
        }
    } else {
        let id = create_child(
            state,
            &snapshot,
            &parent,
            &command,
            &description,
            &agent.name,
            child_model.clone(),
        )
        .await?;
        (id, child_model)
    };
    if crate::tool_runtime::session_is_running(state, &child_session_id).await {
        if continuing {
            let queue_len = crate::tool_runtime::steer_child_task_prompt(
                state,
                &child_session_id,
                &prompt,
                &agent.name,
                child_model,
            )
            .await?;
            return Ok(ToolExecutionResult {
                title: description,
                output: crate::tool_runtime::task_queued_output(
                    &child_session_id,
                    queue_len,
                ),
                metadata: Some(crate::tool_runtime::task_metadata(
                    &child_session_id,
                    &agent.name,
                    "queued",
                    true,
                )),
            });
        }
        return Ok(ToolExecutionResult {
            title: description,
            output: crate::tool_runtime::task_running_output(&child_session_id),
            metadata: Some(crate::tool_runtime::task_metadata(
                &child_session_id,
                &agent.name,
                "running",
                background,
            )),
        });
    }
    if background {
        let generation = Id::ascending(IdKind::Message);
        let admission = if continuing {
            SubtaskAdmissionGuard::admit_continuation(
                state,
                &parent,
                &child_session_id,
            )
            .await
        } else {
            SubtaskAdmissionGuard::admit(
                state,
                &parent,
                &child_session_id,
            )
            .await
        }
        .map_err(|error| error.to_string())?;
        crate::session_actions::mark_subtask_notify_on_idle(
            state,
            &child_session_id,
            &generation,
        )
        .await
        .map_err(|error| error.to_string())?;
        spawn_background_subtask_prompt(
            state.clone(),
            child_session_id.clone(),
            generation,
            prompt,
            agent.name.clone(),
            child_model,
            Some(snapshot.clone()),
            admission,
        );
        return Ok(ToolExecutionResult {
            title: description,
            output: crate::tool_runtime::task_started_output(&child_session_id),
            metadata: Some(crate::tool_runtime::task_metadata(
                &child_session_id,
                &agent.name,
                "running",
                true,
            )),
        });
    }
    let admission = if continuing {
        SubtaskAdmissionGuard::admit_continuation(
            state,
            &parent,
            &child_session_id,
        )
        .await
    } else {
        SubtaskAdmissionGuard::admit(
            state,
            &parent,
            &child_session_id,
        )
        .await
    }
    .map_err(|error| error.to_string())?;
    let result = crate::tool_runtime::run_child_task_prompt_with_cancel(
        state,
        &child_session_id,
        Id::ascending(IdKind::Message),
        &prompt,
        agent.name.clone(),
        child_model,
        cancel,
    )
    .await;
    let result = match result {
        Ok(result) => {
            admission.complete("completed").await;
            result
        }
        Err(error) => {
            admission.complete("failed").await;
            return Err(error.to_string());
        }
    };
    Ok(ToolExecutionResult {
        title: description,
        output: crate::tool_runtime::task_result_output(
            &child_session_id,
            crate::session_actions::last_text_part(&result).unwrap_or_default(),
        ),
        metadata: Some(crate::tool_runtime::task_metadata(
            &child_session_id,
            &agent.name,
            "completed",
            false,
        )),
    })
}

async fn create_child(
    state: &AppState,
    snapshot: &PluginGenerationLease,
    parent: &SessionInfo,
    command: &str,
    description: &str,
    agent: &str,
    model: Option<neoism_agent_core::UserModel>,
) -> Result<String, String> {
    let child = create_subtask_session(state, parent, command, description, agent, model)
        .await
        .map_err(|error| error.to_string())?;
    snapshot
        .subagents()
        .map_err(|error| error.to_string())?
        .track(child.id.to_string())
        .await;
    Ok(child.id.to_string())
}

fn string_arg(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_string)
}

fn uses_configured_harness(input: &Value) -> bool {
    string_arg(input, "subagent_type").is_none_or(|name| name.trim().is_empty() || name == "default")
}

fn native_task_model(
    saved: Option<&SubagentDefaultConfig>,
    input: &Value,
    continuing: bool,
    inherited: Option<&ModelRef>,
) -> Result<Option<UserModel>, String> {
    let inherited = inherited.map(crate::user_model_from_model_ref);
    let Some(saved) = saved.filter(|saved| {
        !continuing && uses_configured_harness(input) && saved.provider == "neoism"
    }) else {
        return Ok(inherited);
    };
    let mut model = match saved.model.as_deref() {
        Some(value) => Some(crate::user_model_from_model_ref(&native_model_ref(value)
            .ok_or("Configured Neoism subagent model must use provider/model syntax")?)),
        None => inherited,
    };
    if let Some(reasoning) = &saved.reasoning {
        model.get_or_insert_with(crate::model_selection::default_user_model).variant = Some(reasoning.clone());
    }
    Ok(model)
}

fn apply_saved_external_options(
    selection: &SubagentDefaultConfig,
    options: &mut Option<BTreeMap<String, String>>,
) -> Result<(), String> {
    let model_id = selection.option_id.as_deref().filter(|id| !id.is_empty());
    let explicit_model = options.as_ref().and_then(|options| {
        model_id.and_then(|id| options.get(id)).or_else(|| options.get("model"))
    });
    // Reasoning was validated against the saved model (or harness default).
    // Never attach that option to a different explicitly requested model.
    let use_saved_reasoning = explicit_model.is_none() || explicit_model == selection.model.as_ref();
    if let Some(model) = selection.model.as_ref().filter(|_| explicit_model.is_none()) {
        let id = model_id.ok_or("Configured subagent model has no validated selector; choose it again")?;
        options.get_or_insert_with(BTreeMap::new).insert(id.into(), model.clone());
    }
    if let Some(reasoning) = selection.reasoning.as_ref().filter(|_| use_saved_reasoning) {
        let id = selection.reasoning_option_id.as_deref().filter(|id| !id.is_empty())
            .ok_or("Configured subagent reasoning has no validated selector; choose it again")?;
        options.get_or_insert_with(BTreeMap::new).entry(id.into()).or_insert_with(|| reasoning.clone());
    }
    Ok(())
}

#[test]
fn subagent_picker_applies_only_missing_compatible_external_options() {
    let selection = SubagentDefaultConfig {
        provider: "cursor".into(), model: Some("saved".into()), option_id: Some("model_id".into()),
        reasoning: Some("high".into()), reasoning_option_id: Some("depth".into()),
    };
    let mut choices = Some(BTreeMap::from([("mode".into(), "safe".into())]));
    apply_saved_external_options(&selection, &mut choices).unwrap();
    let choices = choices.unwrap();
    assert_eq!(choices["model_id"], "saved");
    assert_eq!(choices["depth"], "high");
    assert_eq!(choices["mode"], "safe");
    let mut explicit = Some(BTreeMap::from([("model_id".into(), "explicit".into())]));
    apply_saved_external_options(&selection, &mut explicit).unwrap();
    let explicit = explicit.unwrap();
    assert_eq!(explicit["model_id"], "explicit");
    assert!(!explicit.contains_key("depth"));
    let mut explicit = Some(BTreeMap::from([("depth".into(), "low".into())]));
    apply_saved_external_options(&selection, &mut explicit).unwrap();
    assert_eq!(explicit.unwrap()["depth"], "low");
    assert!(uses_configured_harness(&json!({"subagent_type": "default"})));
    assert!(!uses_configured_harness(&json!({"subagent_type": "cursor"})));
}

#[test]
fn subagent_picker_native_defaults_only_apply_to_new_unqualified_tasks() {
    let mut saved = SubagentDefaultConfig {
        provider: "neoism".into(), model: Some("host/vendor/model".into()), option_id: None,
        reasoning: Some("high".into()), reasoning_option_id: None,
    };
    let inherited = ModelRef {
        provider_id: "parent-host".into(), id: "parent-model".into(),
        connection_id: Some("connection".into()), variant: Some("low".into()),
    };
    let model = native_task_model(Some(&saved), &json!({"subagent_type":"default"}), false, Some(&inherited))
        .unwrap().unwrap();
    assert_eq!(model.provider_id, "host");
    assert_eq!(model.model_id, "vendor/model");
    assert_eq!(model.variant.as_deref(), Some("high"));
    assert!(model.connection_id.is_none());

    for (input, continuing) in [(json!({"subagent_type":"general"}), false), (json!({}), true)] {
        let model = native_task_model(Some(&saved), &input, continuing, Some(&inherited)).unwrap().unwrap();
        assert_eq!(model.model_id, inherited.id);
        assert_eq!(model.variant, inherited.variant);
    }
    saved.model = None;
    let model = native_task_model(Some(&saved), &json!({}), false, Some(&inherited)).unwrap().unwrap();
    assert_eq!(model.model_id, inherited.id);
    assert_eq!(model.connection_id, inherited.connection_id);
    assert_eq!(model.variant.as_deref(), Some("high"));
}

#[tokio::test]
async fn subagent_picker_persists_only_in_the_selected_workspace() {
    use neoism_agent_service_api::{AgentServices, StandardConfigSourceService, StandardExecutableService};

    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let user = temp.path().join("user");
    for directory in [&first, &second, &user] {
        std::fs::create_dir(directory).unwrap();
    }
    let host_settings = user.join("agent.json");
    std::fs::write(&host_settings, r#"{"defaultSubagentHarness":"claude-code"}"#).unwrap();
    let project = first.join(neoism_agent_service_api::STANDARD_AGENT_PROJECT_DIRECTORY);
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join(neoism_agent_service_api::STANDARD_AGENT_CONFIG_FILENAME),
        r#"{"provider":{"picker-test":{"options":{"baseURL":"http://localhost:9999/v1"},"models":{"vendor/model":{}}}}}"#,
    ).unwrap();
    let services = AgentServices::new(Arc::new(StandardExecutableService), crate::standard_workspace_search())
        .with_config(Arc::new(StandardConfigSourceService::new(user)));
    let state = AppState::open_database_with_services(&temp.path().join("agent.db"), services).await.unwrap();
    let query = |directory: &std::path::Path| Query(DefaultDirectory { directory: directory.to_str().unwrap().into() });

    let Json(saved) = set_default(State(state.clone()), query(&first), HeaderMap::new(), None,
        Json(serde_json::from_value(json!({"provider":"neoism","model":"picker-test/vendor/model","reasoning":"high"})).unwrap())).await.unwrap();
    assert_eq!(saved.provider, "neoism");
    for invalid in ["bare-model", "picker-test/", "picker-test/unknown", "missing-provider/vendor/model"] {
        let result = set_default(State(state.clone()), query(&first), HeaderMap::new(), None,
            Json(DefaultSelection { provider: "neoism".into(), model: Some(invalid.into()), reasoning: None })).await;
        assert_eq!(result.err().expect("invalid model must be rejected").into_plugin_response().status, 400);
    }
    let Json(other) = get_default(State(state.clone()), query(&second), HeaderMap::new(), None).await.unwrap();
    assert_eq!(other.provider, "claude");
    let Json(reloaded) = get_default(State(state.clone()), query(&first), HeaderMap::new(), None).await.unwrap();
    assert_eq!(reloaded.provider, "neoism");
    assert_eq!(reloaded.model.as_deref(), Some("picker-test/vendor/model"));
    assert_eq!(reloaded.reasoning.as_deref(), Some("high"));
    let _ = set_default(State(state.clone()), query(&first), HeaderMap::new(), None,
        Json(serde_json::from_value(json!({"provider":"neoism","model":null})).unwrap())).await.unwrap();
    let Json(cleared) = get_default(State(state), query(&first), HeaderMap::new(), None).await.unwrap();
    assert!(cleared.model.is_none());
    assert!(cleared.reasoning.is_none());
    assert_eq!(std::fs::read_to_string(host_settings).unwrap(), r#"{"defaultSubagentHarness":"claude-code"}"#);
    assert!(!second.join(".agent/agent.json").exists());
}

#[cfg(unix)]
#[test]
fn subagent_picker_rejects_a_symlinked_workspace_config_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join(".neoism")).unwrap();
    assert!(guard_config_paths(workspace.path()).is_err());
}

pub(crate) async fn list_tasks(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<SubagentTaskInfo>>, ApiError> {
    let parent = parent_session(&state, &session_id).await?;
    require_enabled(&state, &parent.directory)
        .await
        .map_err(ApiError::not_found)?;
    let mut children =
        crate::tool_runtime::descendant_sessions(&state, parent.id.as_str())
            .await
            .map_err(ApiError::internal)?;
    children.sort_by(|left, right| right.time.updated.cmp(&left.time.updated));
    let mut tasks = Vec::with_capacity(children.len());
    for child in children {
        tasks.push(task_info(&state, &parent, &child).await?);
    }
    Ok(Json(tasks))
}

pub(crate) async fn stop_tasks(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<StopSubagentsRequest>,
) -> Result<Json<StopSubagentsResult>, ApiError> {
    let parent = parent_session(&state, &session_id).await?;
    require_enabled(&state, &parent.directory)
        .await
        .map_err(ApiError::not_found)?;
    let result = stop(&state, &parent, request.task_id.as_deref())
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(result))
}

pub(crate) async fn task_result_tool(
    state: &AppState,
    snapshot: &PluginGenerationLease,
    session_id: &Id,
    input: Value,
) -> Result<ToolExecutionResult, String> {
    let parent = parent_session(state, session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    plugins::enabled(snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "plugin {} is disabled for this workspace",
                neoism_agent_builtins::plugin::subagents::ID
            )
        })?;
    if let Some(task_id) = input.get("task_id").and_then(Value::as_str) {
        let child = child_session(state, task_id).await?;
        crate::tool_runtime::ensure_child_task_belongs_to_parent(state, &parent, &child)
            .await?;
        let (status, output) =
            crate::tool_runtime::task_result_output_for_child(state, &child).await?;
        return Ok(ToolExecutionResult {
            title: child.title,
            output,
            metadata: Some(json!({
                "sessionId": task_id,
                "agent": child.agent,
                "status": status,
            })),
        });
    }

    let mut children =
        crate::tool_runtime::descendant_sessions(state, parent.id.as_str()).await?;
    children.sort_by(|left, right| right.time.updated.cmp(&left.time.updated));
    if children.is_empty() {
        return Ok(ToolExecutionResult {
            title: "Subagent tasks".to_string(),
            output: "No subagent tasks exist for this session yet.".to_string(),
            metadata: Some(json!({ "tasks": [] })),
        });
    }
    let mut lines =
        vec!["Subagent tasks for this session (including nested subagents):".to_string()];
    let mut metadata = Vec::new();
    for child in children {
        let status = crate::tool_runtime::task_status_for_child(state, &child).await?;
        let agent = child.agent.as_deref().unwrap_or("subagent");
        let nested =
            child.parent_id.as_ref().map(|id| id.as_str()) != Some(parent.id.as_str());
        lines.push(format!(
            "task_id: {} status: {} agent: {} title: {}{}",
            child.id,
            status,
            agent,
            child.title,
            if nested { " (nested)" } else { "" }
        ));
        metadata.push(json!({
            "sessionId": child.id,
            "agent": child.agent,
            "status": status,
            "title": child.title,
            "nested": nested,
        }));
    }
    Ok(ToolExecutionResult {
        title: "Subagent tasks".to_string(),
        output: lines.join("\n"),
        metadata: Some(json!({ "tasks": metadata })),
    })
}

pub(crate) async fn stop_task_tool(
    state: &AppState,
    snapshot: &PluginGenerationLease,
    session_id: &Id,
    input: Value,
) -> Result<ToolExecutionResult, String> {
    let parent = parent_session(state, session_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    plugins::enabled(snapshot, neoism_agent_builtins::plugin::subagents::ID)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "plugin {} is disabled for this workspace",
                neoism_agent_builtins::plugin::subagents::ID
            )
        })?;
    let task_id = input.get("task_id").and_then(Value::as_str);
    let result = stop(state, &parent, task_id).await?;
    let count = result.stopped.len();
    Ok(ToolExecutionResult {
        title: "Stopped subagents".to_string(),
        output: if count == 0 {
            "No running subagents to stop for this session.".to_string()
        } else {
            format!(
                "Stopped {count} running subagent(s), including nested tasks. Cleared {} queued prompt(s).",
                result.cleared_prompts
            )
        },
        metadata: Some(json!({
            "stopped": result.stopped,
            "clearedQueue": result.cleared_prompts,
        })),
    })
}

async fn stop(
    state: &AppState,
    parent: &SessionInfo,
    task_id: Option<&str>,
) -> Result<StopSubagentsResult, String> {
    let roots = if let Some(task_id) = task_id {
        let child = child_session(state, task_id).await?;
        crate::tool_runtime::ensure_child_task_belongs_to_parent(state, parent, &child)
            .await?;
        if child.extra.get("externalAgent").is_some_and(|external| external["nativeProjection"] == true || external["nested"] == true) {
            return Err("Provider-native nested tasks are observed only; stop their owning external task instead".into());
        }
        vec![child]
    } else {
        crate::tool_runtime::descendant_sessions(state, parent.id.as_str()).await?
    };
    let mut candidates = Vec::new();
    for root in roots {
        candidates.push(root.clone());
        if task_id.is_some() {
            candidates.extend(
                crate::tool_runtime::descendant_sessions(state, root.id.as_str()).await?,
            );
        }
    }
    candidates.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    candidates.dedup_by(|left, right| left.id == right.id);

    let mut stopped = Vec::new();
    let mut cleared_prompts = 0;
    for child in candidates {
        // Abort also owns durable branch/timer cleanup when the live run vanished.
        let aborted = crate::session_actions::abort_session_run(state, child.id.as_str()).await;
        let cleared =
            crate::session_queue::clear_session_prompt_queue(state, child.id.as_str())
                .await;
        cleared_prompts += cleared;
        if aborted || cleared > 0 {
            stopped.push(child.id.to_string());
        }
    }
    Ok(StopSubagentsResult {
        stopped,
        cleared_prompts,
    })
}

async fn task_info(
    state: &AppState,
    parent: &SessionInfo,
    child: &SessionInfo,
) -> Result<SubagentTaskInfo, ApiError> {
    let (status, output) =
        crate::tool_runtime::task_result_output_for_child(state, child)
            .await
            .map_err(ApiError::internal)?;
    let result = matches!(status.as_str(), "completed" | "error").then_some(output);
    Ok(SubagentTaskInfo {
        id: child.id.to_string(),
        session_id: parent.id.to_string(),
        child_session_id: child.id.to_string(),
        agent: child
            .agent
            .clone()
            .unwrap_or_else(|| "subagent".to_string()),
        status,
        description: child.title.clone(),
        result,
        nested: child.parent_id.as_ref() != Some(&parent.id),
    })
}

async fn parent_session(
    state: &AppState,
    session_id: &str,
) -> Result<SessionInfo, ApiError> {
    state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))
}

async fn child_session(state: &AppState, task_id: &str) -> Result<SessionInfo, String> {
    state
        .inner
        .store
        .get_session(task_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("task_id {task_id} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_config_can_disable_subagents() {
        let mut config = neoism_agent_core::AgentConfigDocument::default();
        config.plugins.insert(
            neoism_agent_builtins::plugin::subagents::ID.to_string(),
            neoism_agent_core::PluginConfig {
                enabled: false,
                ..Default::default()
            },
        );
        assert!(!enabled_in_config(&config));
        config
            .plugins
            .get_mut(neoism_agent_builtins::plugin::subagents::ID)
            .unwrap()
            .enabled = true;
        assert!(enabled_in_config(&config));
    }
}
