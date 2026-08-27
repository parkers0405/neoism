use neoism_agent_plugin_api::{
    PluginContributions, PluginFuture, PluginRuntimeError,
};

pub(crate) struct Skills(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::skills::SkillsHost for Skills {
    fn register_tools(&self, registrar: &mut PluginContributions) {
        crate::tool::register_skill_tools(registrar, &self.0);
    }
}

pub(crate) struct WorkspaceTools(pub(crate) crate::state::AppState);
impl neoism_agent_builtins::plugin::workspace_tools::WorkspaceToolsHost for WorkspaceTools {
    fn register_tools(&self, registrar: &mut PluginContributions) { crate::tool::register_workspace_tools(registrar, &self.0); }
}

pub(crate) struct NotesTools(pub(crate) crate::state::AppState);
impl neoism_agent_builtins::plugin::notes_tools::NotesToolsHost for NotesTools {
    fn register_tools(&self, registrar: &mut PluginContributions) { crate::tool::register_notes_tools(registrar, &self.0); }
}

pub(crate) struct CustomTools(pub(crate) Vec<crate::custom_tool::CustomTool>);
impl neoism_agent_builtins::plugin::custom_tools::CustomToolsHost for CustomTools {
    fn register_tools(&self, registrar: &mut PluginContributions) {
        for tool in &self.0 {
            let item = tool.item();
            registrar.tool(item.id, Some(item.parameters));
        }
    }
}

pub(crate) struct Subagents(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::subagents::SubagentsHost for Subagents {
    fn register_tools(&self, registrar: &mut PluginContributions) {
        crate::tool::register_subagent_tools(registrar, &self.0);
    }

    fn execute<'a>(&'a self, action: neoism_agent_builtins::plugin::subagents::SubagentAction, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Path, State};
            use axum::Json;
            use neoism_agent_builtins::plugin::subagents::SubagentAction;
            let session_id = request.path.get("session_id").cloned().unwrap_or_default();
            let value = match action {
                SubagentAction::List => serde_json::to_value(
                    crate::plugins::subagents::list_tasks(State(self.0.clone()), Path(session_id)).await.map_err(api_error)?.0,
                ),
                SubagentAction::Stop => {
                    let body = serde_json::from_value(request.body).map_err(runtime_error)?;
                    serde_json::to_value(
                        crate::plugins::subagents::stop_tasks(State(self.0.clone()), Path(session_id), Json(body)).await.map_err(api_error)?.0,
                    )
                }
            }.map_err(runtime_error)?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, value))
        })
    }
}

pub(crate) struct Lsp(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::lsp::LspHost for Lsp {
    fn register_tools(&self, registrar: &mut PluginContributions) {
        crate::tool::register_lsp_tools(registrar, &self.0);
    }

    fn execute<'a>(&'a self, action: neoism_agent_builtins::plugin::lsp::LspAction, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Query, State};
            use axum::Json;
            use neoism_agent_builtins::plugin::lsp::LspAction;
            let query = route_query(&request);
            let state = State(self.0.clone());
            let headers = axum::http::HeaderMap::new();
            let value = match action {
                LspAction::Status => serde_json::to_value(crate::lsp_routes::lsp_status(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::Hover => serde_json::to_value(crate::lsp_routes::lsp_hover(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::SignatureHelp => serde_json::to_value(crate::lsp_routes::lsp_signature_help(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::InlayHints => serde_json::to_value(crate::lsp_routes::lsp_inlay_hints(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::DocumentHighlights => serde_json::to_value(crate::lsp_routes::lsp_document_highlights(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::Definition => serde_json::to_value(crate::lsp_routes::lsp_definition(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::References => serde_json::to_value(crate::lsp_routes::lsp_references(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::Implementation => serde_json::to_value(crate::lsp_routes::lsp_implementation(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::PrepareCallHierarchy => serde_json::to_value(crate::lsp_routes::lsp_prepare_call_hierarchy(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::IncomingCalls => serde_json::to_value(crate::lsp_routes::lsp_incoming_calls(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::OutgoingCalls => serde_json::to_value(crate::lsp_routes::lsp_outgoing_calls(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::Diagnostics => serde_json::to_value(crate::lsp_routes::lsp_diagnostics(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::DocumentSymbols => serde_json::to_value(crate::lsp_routes::lsp_document_symbols(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::Formatting => serde_json::to_value(crate::lsp_routes::lsp_formatting(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::CodeActions => serde_json::to_value(crate::lsp_routes::lsp_code_actions(state, Query(query_value(query)?), headers).await.map_err(runtime_error)?.0),
                LspAction::Touch => {
                    let body = serde_json::from_value(request.body).map_err(runtime_error)?;
                    serde_json::to_value(crate::lsp_routes::lsp_touch(state, headers, Json(body)).await.map_err(runtime_error)?.0)
                }
                LspAction::Shutdown => serde_json::to_value(crate::lsp_routes::lsp_shutdown(state).await.0),
            }.map_err(runtime_error)?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, value))
        })
    }
}

pub(crate) struct Mcp(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::mcp::McpHost for Mcp {
    fn register_tools(&self, registrar: &mut PluginContributions) {
        registrar.tool("execute", None);
    }

    fn execute<'a>(&'a self, action: neoism_agent_builtins::plugin::mcp::McpAction, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Path, Query, State};
            use axum::Json;
            use neoism_agent_builtins::plugin::mcp::McpAction;
            let query = route_query(&request);
            let state = State(self.0.clone());
            let headers = axum::http::HeaderMap::new();
            let name = request.path.get("name").cloned().unwrap_or_default();
            if matches!(action, McpAction::AuthCallbackGet) {
                let response = crate::mcp_routes::mcp_auth_callback_get(
                    state, Path(name), Query(query_value(query)?), headers,
                ).await.map_err(api_error)?;
                let mut response = neoism_agent_plugin_api::RouteResponse::json(200, serde_json::Value::String(response.0));
                response.headers.insert("content-type".into(), "text/html; charset=utf-8".into());
                return Ok(response);
            }
            let value = match action {
                McpAction::Status => serde_json::to_value(crate::mcp_routes::mcp_status(state, Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::Add => {
                    let body = serde_json::from_value(request.body).map_err(runtime_error)?;
                    serde_json::to_value(crate::mcp_routes::mcp_add(Json(body)).await.0)
                }
                McpAction::Catalog => serde_json::to_value(crate::mcp_routes::mcp_catalog(state, Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::AuthStart => serde_json::to_value(crate::mcp_routes::mcp_auth_start(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::AuthRemove => serde_json::to_value(crate::mcp_routes::mcp_auth_remove(state, Query(query_value(query)?), Path(name), headers).await.map_err(api_error)?.0),
                McpAction::AuthCallbackPost => {
                    let body = serde_json::from_value(request.body).map_err(runtime_error)?;
                    serde_json::to_value(crate::mcp_routes::mcp_auth_callback(state, Path(name), Query(query_value(query)?), headers, Json(body)).await.map_err(api_error)?.0)
                }
                McpAction::Authenticate => serde_json::to_value(crate::mcp_routes::mcp_auth_authenticate(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::Connect => serde_json::to_value(crate::mcp_routes::mcp_connect(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::Disconnect => serde_json::to_value(crate::mcp_routes::mcp_disconnect(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::Config => {
                    let body = serde_json::from_value(request.body).map_err(runtime_error)?;
                    serde_json::to_value(crate::mcp_routes::mcp_config_patch(state, Path(name), Query(query_value(query)?), headers, Json(body)).await.map_err(api_error)?.0)
                }
                McpAction::Tools => serde_json::to_value(crate::mcp_routes::mcp_tools(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::ToolCall => {
                    let tool_name = request.path.get("tool_name").cloned().unwrap_or_default();
                    serde_json::to_value(crate::mcp_routes::mcp_tool_call(state, Path((name, tool_name)), Query(query_value(query)?), headers, Json(request.body)).await.map_err(api_error)?.0)
                }
                McpAction::Resources => serde_json::to_value(crate::mcp_routes::mcp_resources(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::Prompts => serde_json::to_value(crate::mcp_routes::mcp_prompts(state, Path(name), Query(query_value(query)?), headers).await.map_err(api_error)?.0),
                McpAction::AuthCallbackGet => unreachable!(),
            }.map_err(runtime_error)?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, value))
        })
    }
}

pub(crate) struct Pty(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::pty::PtyHost for Pty {
    fn execute<'a>(&'a self, action: neoism_agent_builtins::plugin::pty::PtyAction, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Path, Query, State};
            use axum::Json;
            use neoism_agent_builtins::plugin::pty::PtyAction;
            let query = route_query(&request);
            let state = State(self.0.clone());
            let headers = header_map(&request.headers);
            let pty_id = request.path.get("pty_id").cloned().unwrap_or_default();
            let value = match action {
                PtyAction::Shells => serde_json::to_value(crate::pty_routes::pty_shells().await.0),
                PtyAction::List => serde_json::to_value(crate::pty_routes::pty_list(state).await.0),
                PtyAction::Create => serde_json::to_value(crate::pty_routes::pty_create(state, Query(query_value(query)?), headers, Json(request.body)).await.map_err(api_error)?.0),
                PtyAction::Get => serde_json::to_value(crate::pty_routes::pty_get(state, Path(pty_id)).await.map_err(api_error)?.0),
                PtyAction::Update => {
                    let body = serde_json::from_value(request.body).map_err(runtime_error)?;
                    serde_json::to_value(crate::pty_routes::pty_update(state, Path(pty_id), Json(body)).await.map_err(api_error)?.0)
                }
                PtyAction::Remove => serde_json::to_value(crate::pty_routes::pty_remove(state, Path(pty_id)).await.map_err(api_error)?.0),
                PtyAction::ConnectToken => serde_json::to_value(crate::pty_routes::pty_connect_token(state, Path(pty_id), headers).await.map_err(api_error)?.0),
            }.map_err(runtime_error)?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, value))
        })
    }

    fn connect<'a>(&'a self, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, std::sync::Arc<dyn neoism_agent_plugin_api::WebSocketSession>> {
        Box::pin(async move {
            let pty_id = request.path.get("pty_id").cloned().unwrap_or_default();
            let query = query_value(route_query(&request))?;
            let generation = request_generation(&self.0, &request).await
                .ok_or_else(|| neoism_agent_plugin_api::PluginRuntimeError::new("plugin generation is no longer available"))?;
            crate::pty_routes::prepare_connection_with_runtime(
                self.0.clone(),
                generation.pty()?,
                pty_id,
                query,
            )
            .await
            .map_err(api_error)
        })
    }
}

async fn request_generation(
    state: &crate::state::AppState,
    request: &neoism_agent_plugin_api::RouteRequest,
) -> Option<crate::workspace_runtime::PluginGenerationLease> {
    let workspace = request.workspace.as_ref()?;
    let generation = request.generation?;
    if let Some(active) = crate::workspace_runtime::active_generation(&workspace.to_string_lossy()) {
        if active.generation == generation {
            return Some(active);
        }
    }
    state
        .inner
        .workspace_runtimes
        .loaded(&workspace.to_string_lossy()).await?
        .lease_generation(generation)
}

pub(crate) struct ConfigAdmin(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::config::ConfigAdminHost for ConfigAdmin {
    fn execute<'a>(&'a self, action: neoism_agent_builtins::plugin::config::ConfigAdminAction, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use neoism_agent_builtins::plugin::config::ConfigAdminAction;
            let directory = request.workspace.unwrap_or_default();
            let directory = directory.to_string_lossy();
            let body = match action {
                ConfigAdminAction::Defaults => {
                    let config = neoism_agent_builtins::plugin::config::load(self.0.services(), &directory).map_err(runtime_error)?.0;
                    Ok(neoism_agent_builtins::plugin::config::selection_defaults(&config))
                }
                ConfigAdminAction::Get => {
                    let mut config = neoism_agent_builtins::plugin::config::load(self.0.services(), &directory).map_err(runtime_error)?.0;
                    crate::config::inject_builtin_mcp(&mut config, self.0.services());
                    serde_json::to_value(config)
                }
                ConfigAdminAction::Validate => serde_json::to_value(crate::config::validate(self.0.services(), &directory)),
                ConfigAdminAction::Update => {
                    let config: neoism_agent_core::AgentConfigDocument = serde_json::from_value(request.body).map_err(runtime_error)?;
                    let snapshot = crate::config::snapshot(self.0.services(), &directory).map_err(runtime_error)?;
                    self.0.services().config.update(&neoism_agent_service_api::ConfigUpdateRequest {
                        workspace: std::path::PathBuf::from(directory.as_ref()),
                        source_id: snapshot.writable_target.source_id,
                        update: neoism_agent_service_api::ConfigUpdate::ReplaceDocument {
                            document: serde_json::to_value(&config).map_err(runtime_error)?,
                        },
                    }).await.map_err(runtime_error)?;
                    serde_json::to_value(config)
                }
            }.map_err(runtime_error)?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, body))
        })
    }
}

pub(crate) struct Semantic(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::semantic::SemanticHost for Semantic {
    fn search<'a>(&'a self, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Query, State};
            let query = request.query.into_iter().fold(serde_json::Map::new(), |mut output, (key, values)| {
                output.insert(key, serde_json::json!(values.first().cloned().unwrap_or_default()));
                output
            });
            let query = serde_json::from_value(serde_json::Value::Object(query)).map_err(runtime_error)?;
            let response = crate::semantic::semantic_search_route(State(self.0.clone()), Query(query)).await.map_err(api_error)?;
            let body = serde_json::to_value(response.0).map_err(runtime_error)?;
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, body))
        })
    }
}

pub(crate) struct Workflows(pub(crate) crate::state::AppState);

impl neoism_agent_builtins::plugin::workflows::WorkflowsHost for Workflows {
    fn execute<'a>(&'a self, action: neoism_agent_builtins::plugin::workflows::WorkflowAction, request: neoism_agent_plugin_api::RouteRequest) -> PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
        Box::pin(async move {
            use axum::extract::{Path, Query, State};
            use neoism_agent_builtins::plugin::workflows::WorkflowAction;
            let query = route_query(&request);
            let state = State(self.0.clone());
            let headers = axum::http::HeaderMap::new();
            let workflow_id = request.path.get("workflow_id").cloned().unwrap_or_default();
            let value = match action {
                WorkflowAction::List => crate::workflow::workflow_list(state, Query(query_value(query)?), headers).await.map_err(api_error)?.0,
                WorkflowAction::Get => crate::workflow::workflow_get(state, Query(query_value(query)?), headers, Path(workflow_id)).await.map_err(api_error)?.0,
                WorkflowAction::Activate => crate::workflow::workflow_activate(state, Query(query_value(query)?), headers, Path(workflow_id)).await.map_err(api_error)?.0,
                WorkflowAction::Pause => crate::workflow::workflow_pause(state, Query(query_value(query)?), headers, Path(workflow_id)).await.map_err(api_error)?.0,
                WorkflowAction::Run => crate::workflow::workflow_run_now(state, Query(query_value(query)?), headers, Path(workflow_id)).await.map_err(api_error)?.0,
                WorkflowAction::Preview => crate::workflow::workflow_preview(state, Query(query_value(query)?), headers, Path(workflow_id)).await.map_err(api_error)?.0,
                WorkflowAction::History => crate::workflow::workflow_history(state, Query(query_value(query)?), headers, Path(workflow_id)).await.map_err(api_error)?.0,
            };
            Ok(neoism_agent_plugin_api::RouteResponse::json(200, value))
        })
    }
}

pub(crate) struct Artifacts(pub(crate) crate::state::AppState);
impl neoism_agent_builtins::plugin::artifacts::ArtifactsHost for Artifacts {
    fn register_tools(&self, registrar: &mut PluginContributions) { crate::tool::register_artifact_tools(registrar, &self.0); }
}

pub(crate) struct Interactions(pub(crate) crate::state::AppState);
impl neoism_agent_builtins::plugin::interactions::InteractionsHost for Interactions {
    fn register_tools(&self, registrar: &mut PluginContributions) { crate::tool::register_interaction_tools(registrar, &self.0); }
}

pub(crate) struct Goals(pub(crate) crate::state::AppState);
impl neoism_agent_builtins::plugin::goals::GoalsHost for Goals {
    fn register_tools(&self, registrar: &mut PluginContributions) { crate::tool::register_goal_tools(registrar, &self.0); }

    fn load<'a>(&'a self, session_id: &'a str) -> PluginFuture<'a, Option<neoism_agent_core::SessionGoal>> {
        Box::pin(async move { crate::ensure_session(&self.0, session_id).await.map(|session| session.goal()).map_err(runtime_error) })
    }

    fn save<'a>(&'a self, session_id: &'a str, goal: Option<neoism_agent_core::SessionGoal>) -> PluginFuture<'a, Option<neoism_agent_core::SessionGoal>> {
        Box::pin(async move {
            let mut session = crate::ensure_session(&self.0, session_id).await.map_err(runtime_error)?;
            if let Some(goal) = &goal { session.set_goal(goal); } else { session.clear_goal(); }
            session.time.updated = crate::now_millis().max(session.time.updated.saturating_add(1)).max(goal.as_ref().map(|goal| goal.updated).unwrap_or_default());
            self.0.inner.store.update_session(&session).await.map_err(runtime_error)?;
            self.0.publish(neoism_agent_core::EventPayload::new(
                neoism_agent_core::event_type::SESSION_UPDATED,
                serde_json::json!({ "sessionID": session.id.to_string(), "info": session }),
            ));
            Ok(goal)
        })
    }
}

fn route_query(request: &neoism_agent_plugin_api::RouteRequest) -> serde_json::Value {
    let mut query = request.query.iter().fold(serde_json::Map::new(), |mut output, (key, values)| {
        let value = values.first().cloned().unwrap_or_default();
        let value = value.parse::<u64>().map_or_else(|_| serde_json::Value::String(value), |value| serde_json::json!(value));
        output.insert(key.clone(), value);
        output
    });
    if let Some(workspace) = &request.workspace { query.insert("directory".into(), serde_json::json!(workspace)); }
    serde_json::Value::Object(query)
}

fn query_value<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, PluginRuntimeError> {
    serde_json::from_value(value).map_err(runtime_error)
}

fn header_map(headers: &std::collections::BTreeMap<String, String>) -> axum::http::HeaderMap {
    headers.iter().filter_map(|(name, value)| {
        Some((name.parse::<axum::http::HeaderName>().ok()?, value.parse::<axum::http::HeaderValue>().ok()?))
    }).collect()
}

fn api_error(error: crate::error::ApiError) -> PluginRuntimeError { PluginRuntimeError::new(error.to_string()) }
fn runtime_error(error: impl std::fmt::Display) -> PluginRuntimeError { PluginRuntimeError::new(error.to_string()) }
