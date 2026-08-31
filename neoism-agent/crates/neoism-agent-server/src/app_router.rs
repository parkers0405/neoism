use axum::body::{to_bytes, Body};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State};
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::artifact_routes::{
    artifact_content, artifact_create, artifact_delete, artifact_get, artifact_list,
};
use crate::audit_routes::audit_list;
use crate::global_routes::global_health;
use crate::interaction::{
    permission_list, permission_reply, question_list, question_reject, question_reply,
};
use crate::openapi::canonical_openapi_doc;
use crate::session_actions::{session_command, session_shell};
use crate::session_export_route::sessions_export;
use crate::session_import_route::session_import;
use crate::session_message_routes::{message_delete, message_get, part_delete, part_update};
use crate::session_prompt_routes::{session_abort, session_summarize};
use crate::session_queue::{session_queue, session_queue_clear, session_queue_pop};
use crate::session_routes::{
    session_create, session_delete, session_diff, session_directory_options, session_fork,
    session_get, session_set_pin, session_status, session_todo_list, session_update,
};
use crate::session_undo::{
    session_redo, session_revert, session_undo, session_undo_tree, session_unrevert,
};
use crate::state::AppState;
use crate::tool_routes::tool_list;
use crate::v2_routes::{
    v2_capabilities, v2_compact, v2_context, v2_events, v2_message_list, v2_meta,
    v2_plugin, v2_plugins, v2_prompt, v2_prompt_async, v2_session_children, v2_session_list,
    v2_session_runtime, v2_wait,
};

pub fn app(state: AppState) -> Router {
    app_with_cors(state, &[])
}

pub(crate) fn app_with_cors(state: AppState, allowed_origins: &[String]) -> Router {
    let middleware_state = state.clone();
    let router = Router::new()
        .route("/v2/health", get(global_health))
        .route("/v2/meta", get(v2_meta))
        .route("/v2/openapi.json", get(canonical_openapi_doc))
        .route("/v2/audit", get(audit_list))
        .route("/v2/capabilities", get(v2_capabilities))
        .route("/v2/plugins", get(v2_plugins))
        .route("/v2/plugins/:plugin_id/manifest", get(v2_plugin))
        .route("/v2/events", get(v2_events))
        .route("/v2/artifacts", get(artifact_list).post(artifact_create))
        .route(
            "/v2/artifacts/:artifact_id",
            get(artifact_get).delete(artifact_delete),
        )
        .route("/v2/artifacts/:artifact_id/content", get(artifact_content))
        .route("/v2/interactions/permissions", get(permission_list))
        .route(
            "/v2/interactions/permissions/:request_id/reply",
            post(permission_reply),
        )
        .route("/v2/interactions/questions", get(question_list))
        .route(
            "/v2/interactions/questions/:request_id/reply",
            post(question_reply),
        )
        .route(
            "/v2/interactions/questions/:request_id/reject",
            post(question_reject),
        )
        .route("/v2/tools", get(tool_list))
        .route("/v2/sessions", get(v2_session_list).post(session_create))
        .route("/v2/sessions/status", get(session_status))
        .route("/v2/sessions/import", post(session_import))
        .route("/v2/sessions/export", post(sessions_export))
        .route(
            "/v2/sessions/:session_id",
            get(session_get).patch(session_update).delete(session_delete),
        )
        .route("/v2/sessions/:session_id/messages", get(v2_message_list))
        .route(
            "/v2/sessions/:session_id/messages/:message_id",
            get(message_get).delete(message_delete),
        )
        .route(
            "/v2/sessions/:session_id/messages/:message_id/parts/:part_id",
            delete(part_delete).patch(part_update),
        )
        .route("/v2/sessions/:session_id/children", get(v2_session_children))
        .route("/v2/sessions/:session_id/runtime", get(v2_session_runtime))
        .route(
            "/v2/sessions/:session_id/directory-options",
            get(session_directory_options),
        )
        .route("/v2/sessions/:session_id/todos", get(session_todo_list))
        .route("/v2/sessions/:session_id/fork", post(session_fork))
        .route("/v2/sessions/:session_id/diff", get(session_diff))
        .route("/v2/sessions/:session_id/undo-tree", get(session_undo_tree))
        .route("/v2/sessions/:session_id/prompt", post(v2_prompt))
        .route("/v2/sessions/:session_id/prompt-async", post(v2_prompt_async))
        .route("/v2/sessions/:session_id/abort", post(session_abort))
        .route("/v2/sessions/:session_id/compact", post(v2_compact))
        .route("/v2/sessions/:session_id/wait", post(v2_wait))
        .route("/v2/sessions/:session_id/context", get(v2_context))
        .route(
            "/v2/sessions/:session_id/queue",
            get(session_queue).delete(session_queue_clear),
        )
        .route("/v2/sessions/:session_id/queue/pop", post(session_queue_pop))
        .route("/v2/sessions/:session_id/commands", post(session_command))
        .route("/v2/sessions/:session_id/shell", post(session_shell))
        .route("/v2/sessions/:session_id/revert", post(session_revert))
        .route("/v2/sessions/:session_id/unrevert", post(session_unrevert))
        .route("/v2/sessions/:session_id/undo", post(session_undo))
        .route("/v2/sessions/:session_id/redo", post(session_redo))
        .route("/v2/sessions/:session_id/summarize", post(session_summarize))
        .route("/v2/sessions/:session_id/pin", post(session_set_pin))
        .route(
            "/v2/sessions/:session_id/jobs/:job_id",
            delete(crate::background_job::stop_background_task),
        )
        .merge(management_routes(&state))
        .fallback(plugin_route_dispatch)
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        target: "neoism_agent::perf",
                        "http_request",
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
                .on_request(DefaultOnRequest::new().level(Level::TRACE))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(middleware::from_fn_with_state(
            middleware_state,
            authenticate_request,
        ));
    if allowed_origins.is_empty() {
        router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
    } else {
        let origins = allowed_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect::<Vec<_>>();
        router.layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(Any)
                .allow_headers(Any),
        )
    }
}

fn management_routes(state: &AppState) -> Router<AppState> {
    if !state.management_enabled() { return Router::new(); }
    use crate::management as management;
    Router::new()
        .route("/v2/management/workspaces", get(management::list_workspaces).post(management::create_workspace))
        .route("/v2/management/workspaces/:id", get(management::get_workspace).put(management::update_workspace).delete(management::delete_workspace))
        .route("/v2/management/repositories", get(management::list_repositories).post(management::create_repository))
        .route("/v2/management/repositories/:id", get(management::get_repository).put(management::update_repository).delete(management::delete_repository))
        .route("/v2/management/agents", get(management::list_agents))
        .route("/v2/management/agents/:id", get(management::get_agent).post(management::create_agent).put(management::update_agent).delete(management::delete_agent))
        .route("/v2/management/commands", get(management::list_commands))
        .route("/v2/management/commands/:id", get(management::get_command).post(management::create_command).put(management::update_command).delete(management::delete_command))
        .route("/v2/management/skills", get(management::list_skills))
        .route("/v2/management/skills/install", post(management::install_skill))
        .route("/v2/management/skills/:id", get(management::get_skill).post(management::create_skill).put(management::update_skill).delete(management::delete_skill))
        .route("/v2/management/skills/:id/versions", get(management::list_skill_versions))
        .route("/v2/management/skills/:id/versions/:version", get(management::get_skill_version))
        .route("/v2/management/skills/:id/versions/:version/restore", post(management::restore_skill_version))
}

async fn plugin_route_dispatch(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let directory = if let Some(directory) = request_directory(&request) {
        directory
    } else if let Some(matched) = request.extensions().get::<MatchedPluginSession>() {
        matched.directory.clone()
    } else if request.uri().path().starts_with("/v2/plugins/") {
        // Descriptor-validated only: a session may resolve the dispatching
        // workspace solely through a route that declares RouteScope::Session
        // and binds this segment as `session_id`. Guessing from the path let a
        // plugin resource id that collides with a session id teleport dispatch
        // into that session's workspace.
        match find_scoped_plugin_session(&state, request.uri().path(), request.method().as_str()).await {
            Some(session) => session.directory,
            None => std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned(),
        }
    } else if let Some(session_id) = session_id_from_path(request.uri().path()) {
        match state.inner.store.get_session(session_id).await {
            Ok(Some(session)) => session.directory,
            _ => return StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned()
    };
    let snapshot = state.plugin_snapshot(&directory).await;
    if !snapshot.is_active() {
        return auth_error(StatusCode::GONE, "plugin.generation_closed", "The plugin generation is shut down");
    }
    let websocket_route = snapshot.runtime_websocket_routes.values().find_map(|registered| {
        (registered.route.descriptor.method.as_str() == request.method().as_str())
            .then(|| match_plugin_path(&registered.route.descriptor.path, request.uri().path()))
            .flatten()
            .map(|params| (registered, params))
    });
    if let Some((registered, path_params)) = websocket_route {
        return dispatch_websocket_route(
            registered,
            path_params,
            directory,
            request,
            snapshot.clone(),
            state.clone(),
        )
        .await;
    }
    let runtime_route = snapshot.runtime_routes.values().find_map(|registered| {
        (registered.route.descriptor.method.as_str() == request.method().as_str())
            .then(|| match_plugin_path(&registered.route.descriptor.path, request.uri().path()))
            .flatten()
            .map(|params| (registered, params))
    });
    if let Some((registered, path_params)) = runtime_route {
        return dispatch_runtime_route(
            registered,
            path_params,
            directory,
            request,
            snapshot.clone(),
            state.clone(),
        )
        .await;
    }
    StatusCode::NOT_FOUND.into_response()
}

#[derive(Clone, Debug)]
struct MatchedPluginSession {
    session_id: String,
    directory: String,
}

/// Find the session a plugin route path is scoped to, accepting a path
/// segment as a session id ONLY when a registered route descriptor declares
/// `RouteScope::Session` and binds that exact segment as `session_id`. A
/// plugin resource id that merely collides with a session id never matches:
/// its route's descriptor binds a different parameter.
async fn find_scoped_plugin_session(
    state: &AppState,
    path: &str,
    method: &str,
) -> Option<neoism_agent_core::SessionInfo> {
    for candidate_id in path.split('/').filter(|segment| !segment.is_empty()).rev() {
        let Some(session) = state.inner.store.get_session(candidate_id).await.ok().flatten() else { continue; };
        let snapshot = state.plugin_snapshot(&session.directory).await;
        if snapshot_matches_session_route(&snapshot, method, path, session.id.as_str()) {
            return Some(session);
        }
    }
    None
}

async fn resolve_scoped_plugin_session(
    state: &AppState,
    path: &str,
    method: &str,
    claims: &crate::caller::CallerClaims,
) -> Result<Option<MatchedPluginSession>, Response> {
    let Some(session) = find_scoped_plugin_session(state, path, method).await else {
        return Ok(None);
    };
    if !allows_session_or_ancestor(state, claims, &session)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to authorize plugin session ancestry");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "auth.lookup_failed",
                "Failed to authorize the session",
            )
        })?
    {
        return Err(auth_error(StatusCode::FORBIDDEN, "auth.session_forbidden", "The caller is not authorized for this session or directory"));
    }
    Ok(Some(MatchedPluginSession { session_id: session.id.to_string(), directory: session.directory }))
}

fn snapshot_matches_session_route(snapshot: &neoism_agent_plugin_api::RegistrySnapshot, method: &str, path: &str, session_id: &str) -> bool {
    snapshot.runtime_routes.values().any(|registered| {
        registered.route.descriptor.scope == neoism_agent_plugin_api::RouteScope::Session
            && registered.route.descriptor.method.as_str() == method
            && match_plugin_path(&registered.route.descriptor.path, path)
                .and_then(|params| params.get("session_id").cloned())
                .as_deref() == Some(session_id)
    }) || snapshot.runtime_websocket_routes.values().any(|registered| {
        registered.route.descriptor.scope == neoism_agent_plugin_api::RouteScope::Session
            && registered.route.descriptor.method.as_str() == method
            && match_plugin_path(&registered.route.descriptor.path, path)
                .and_then(|params| params.get("session_id").cloned())
                .as_deref() == Some(session_id)
    })
}

struct HostWebSocket(WebSocket);

impl neoism_agent_plugin_api::PluginWebSocket for HostWebSocket {
    fn receive<'a>(&'a mut self) -> neoism_agent_plugin_api::PluginFuture<'a, Option<neoism_agent_plugin_api::WebSocketMessage>> {
        Box::pin(async move {
            use neoism_agent_plugin_api::WebSocketMessage as PluginMessage;
            Ok(match self.0.recv().await {
                Some(Ok(Message::Text(value))) => Some(PluginMessage::Text(value.to_string())),
                Some(Ok(Message::Binary(value))) => Some(PluginMessage::Binary(value.to_vec())),
                Some(Ok(Message::Ping(value))) => Some(PluginMessage::Ping(value.to_vec())),
                Some(Ok(Message::Pong(value))) => Some(PluginMessage::Pong(value.to_vec())),
                Some(Ok(Message::Close(_))) | None => Some(PluginMessage::Close),
                Some(Err(error)) => return Err(neoism_agent_plugin_api::PluginRuntimeError::new(error.to_string())),
            })
        })
    }

    fn send<'a>(&'a mut self, message: neoism_agent_plugin_api::WebSocketMessage) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
        Box::pin(async move {
            use neoism_agent_plugin_api::WebSocketMessage as PluginMessage;
            let message = match message {
                PluginMessage::Text(value) => Message::Text(value.into()),
                PluginMessage::Binary(value) => Message::Binary(value.into()),
                PluginMessage::Ping(value) => Message::Ping(value.into()),
                PluginMessage::Pong(value) => Message::Pong(value.into()),
                PluginMessage::Close => Message::Close(None),
            };
            self.0.send(message).await.map_err(|error| neoism_agent_plugin_api::PluginRuntimeError::new(error.to_string()))
        })
    }
}

async fn dispatch_websocket_route(
    registered: &neoism_agent_plugin_api::RegisteredWebSocketRouteContribution,
    path: std::collections::BTreeMap<String, String>,
    directory: String,
    request: Request<Body>,
    generation_lease: crate::workspace_runtime::PluginGenerationLease,
    state: AppState,
) -> Response {
    if generation_lease.ensure_active().is_err() { return auth_error(StatusCode::GONE, "plugin.generation_closed", "The plugin generation is shut down"); }
    if let Err(response) = validate_plugin_route_scope(&registered.route.descriptor, &path, &directory, &state).await { return response; }
    let (mut parts, _) = request.into_parts();
    let query = url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes())
        .fold(std::collections::BTreeMap::<String, Vec<String>>::new(), |mut output, (key, value)| {
            output.entry(key.into_owned()).or_default().push(value.into_owned());
            output
        });
    let headers = parts.headers.iter().filter_map(|(name, value)| value.to_str().ok().map(|value| (name.to_string(), value.to_string()))).collect();
    let claims = parts.extensions.get::<crate::caller::CallerClaims>();
    let route_request = neoism_agent_plugin_api::RouteRequest {
        tenant_id: claims.map(|claims| claims.tenant_id.clone()),
        hosted: claims.is_some_and(|claims| claims.hosted),
        workspace_id: claims.and_then(|claims| claims.workspace_id.clone()),
        workspace: Some(std::path::PathBuf::from(directory)),
        session_id: path.get("session_id").cloned(),
        actor: claims.map(|claims| claims.subject.clone()),
        generation: Some(generation_lease.generation),
        path,
        query,
        headers,
        body: serde_json::Value::Null,
    };
    let session = match crate::workspace_runtime::scope_generation(
        generation_lease.clone(),
        registered.route.handler.prepare(route_request),
    )
    .await
    {
        Ok(session) => session,
        Err(error) => return auth_error(StatusCode::BAD_REQUEST, "plugin.websocket_rejected", &error.to_string()),
    };
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(error) => return error.into_response(),
    };
    let mut cancellation = generation_lease.websocket_cancellation();
    upgrade.on_upgrade(move |socket| async move {
        let _ = crate::workspace_runtime::scope_generation(
            generation_lease,
            async move {
                tokio::select! {
                    result = session.run(Box::new(HostWebSocket(socket))) => result,
                    _ = cancellation.changed() => Ok(()),
                }
            },
        ).await;
    }).into_response()
}

async fn dispatch_runtime_route(
    registered: &neoism_agent_plugin_api::RegisteredRouteContribution,
    path_params: std::collections::BTreeMap<String, String>,
    directory: String,
    request: Request<Body>,
    generation: crate::workspace_runtime::PluginGenerationLease,
    state: AppState,
) -> Response {
    if generation.ensure_active().is_err() { return auth_error(StatusCode::GONE, "plugin.generation_closed", "The plugin generation is shut down"); }
    if let Err(response) = validate_plugin_route_scope(&registered.route.descriptor, &path_params, &directory, &state).await { return response; }
    let (parts, body) = request.into_parts();
    let mut query = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (key, value) in
        url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes())
    {
        query.entry(key.into_owned()).or_default().push(value.into_owned());
    }
    let headers = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect();
    let body = match to_bytes(body, 16 * 1024 * 1024).await {
        Ok(bytes) if bytes.is_empty() => serde_json::Value::Null,
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                return auth_error(
                    StatusCode::BAD_REQUEST,
                    "request.invalid_json",
                    &error.to_string(),
                )
            }
        },
        Err(error) => {
            return auth_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request.body_too_large",
                &error.to_string(),
            )
        }
    };
    let claims = parts
        .extensions
        .get::<crate::caller::CallerClaims>();
    let actor = claims.map(|claims| claims.subject.clone());
    let workspace_id = claims.and_then(|claims| claims.workspace_id.clone()).or_else(|| {
        (!claims.is_some_and(|claims| claims.hosted))
            .then(|| query.get("workspaceId").and_then(|values| values.first()).cloned())
            .flatten()
    });
    let session_id = path_params.get("session_id").cloned();
    let request = neoism_agent_plugin_api::RouteRequest {
        tenant_id: claims.map(|claims| claims.tenant_id.clone()),
        hosted: claims.is_some_and(|claims| claims.hosted),
        workspace_id,
        workspace: Some(std::path::PathBuf::from(directory)),
        session_id,
        actor,
        generation: Some(generation.generation),
        path: path_params,
        query,
        headers,
        body,
    };
    match crate::workspace_runtime::scope_generation(
        generation,
        registered.route.handler.handle(request),
    )
    .await
    {
        Ok(plugin_response) => {
            let status = StatusCode::from_u16(plugin_response.status)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let content_type = plugin_response
                .headers
                .get("content-type")
                .map(String::as_str)
                .unwrap_or("application/json");
            let body = if content_type.starts_with("text/") {
                plugin_response.body.as_str().unwrap_or_default().to_string()
            } else {
                plugin_response.body.to_string()
            };
            let mut response = Response::builder().status(status);
            for (name, value) in plugin_response.headers {
                response = response.header(name, value);
            }
            response.body(Body::from(body)).unwrap_or_else(|error| {
                auth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "plugin.invalid_response",
                    &error.to_string(),
                )
            })
        }
        Err(error) => auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "plugin.route_failed",
            &error.to_string(),
        ),
    }
}

async fn validate_plugin_route_scope(
    descriptor: &neoism_agent_plugin_api::RouteDescriptor,
    path: &std::collections::BTreeMap<String, String>,
    directory: &str,
    state: &AppState,
) -> Result<(), Response> {
    match descriptor.scope {
        neoism_agent_plugin_api::RouteScope::Workspace => Ok(()),
        neoism_agent_plugin_api::RouteScope::Session => {
            let session_id = path.get("session_id").ok_or_else(|| auth_error(
                StatusCode::BAD_REQUEST,
                "plugin.session_scope_required",
                "This plugin route requires a session scope",
            ))?;
            match state.inner.store.get_session(session_id).await {
                Ok(Some(session)) if crate::workspace_runtime::canonical_location(&session.directory)
                    == crate::workspace_runtime::canonical_location(directory) => Ok(()),
                Ok(Some(_)) => Err(auth_error(StatusCode::FORBIDDEN, "plugin.session_workspace_mismatch", "The session does not belong to this workspace")),
                Ok(None) => Err(auth_error(StatusCode::NOT_FOUND, "plugin.session_not_found", "Session not found")),
                Err(error) => Err(auth_error(StatusCode::INTERNAL_SERVER_ERROR, "plugin.session_lookup_failed", &error.to_string())),
            }
        }
    }
}

fn match_plugin_path(
    pattern: &str,
    path: &str,
) -> Option<std::collections::BTreeMap<String, String>> {
    let pattern = pattern.trim_matches('/').split('/').collect::<Vec<_>>();
    let path = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if pattern.len() != path.len() {
        return None;
    }
    let mut params = std::collections::BTreeMap::new();
    for (pattern, value) in pattern.into_iter().zip(path) {
        if let Some(name) = pattern.strip_prefix(':') {
            params.insert(name.to_string(), value.to_string());
        } else if pattern != value {
            return None;
        }
    }
    Some(params)
}

/// Authorize a session directly, or through an authorized ancestor in the
/// same workspace and directory. Older internally-spawned subagent sessions
/// did not copy the tenant marker from their parent, so direct authorization
/// alone makes a legitimately shared child impossible to open.
async fn allows_session_or_ancestor(
    state: &AppState,
    claims: &crate::caller::CallerClaims,
    session: &neoism_agent_core::SessionInfo,
) -> anyhow::Result<bool> {
    if crate::caller::allows_session(claims, session) {
        return Ok(true);
    }
    if !crate::caller::allows_directory(claims, &session.directory) {
        return Ok(false);
    }
    if claims.workspace_id.as_ref().is_some_and(|workspace_id| {
        session
            .workspace_id
            .as_ref()
            .is_none_or(|session_workspace| session_workspace.to_string() != *workspace_id)
    }) {
        return Ok(false);
    }

    let descendant_directory = crate::workspace_runtime::canonical_location(&session.directory);
    let descendant_workspace = session.workspace_id.clone();
    let mut parent_id = session.parent_id.clone();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..16 {
        let Some(id) = parent_id else { return Ok(false) };
        if !seen.insert(id.to_string()) {
            return Ok(false);
        }
        let Some(parent) = state.inner.store.get_session(id.as_str()).await? else {
            return Ok(false);
        };
        if parent.workspace_id != descendant_workspace
            || crate::workspace_runtime::canonical_location(&parent.directory)
                != descendant_directory
        {
            return Ok(false);
        }
        if crate::caller::allows_session(claims, &parent) {
            return Ok(true);
        }
        parent_id = parent.parent_id;
    }
    Ok(false)
}

async fn authenticate_request(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS
        || request.uri().path() == "/v2/health"
        || is_ticketed_pty_connect(&request)
        || is_mcp_oauth_callback_get(&request)
    {
        return next.run(request).await;
    }
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let claims = match state.inner.caller_policy.authenticate(supplied) {
        Ok(claims) => claims,
        Err(message) => return auth_error(StatusCode::UNAUTHORIZED, "auth.invalid_token", &message),
    };
    let mut request_guard = None;
    let mut audit_tenant = None;
    let mut audit_subject = None;
    if let Some(claims) = claims {
        request_guard = match state.inner.caller_policy.begin_request(&claims) {
            Ok(guard) => Some(guard),
            Err(message) => {
                return auth_error(StatusCode::TOO_MANY_REQUESTS, "quota.exceeded", message)
            }
        };
        audit_tenant = Some(claims.tenant_id.clone());
        audit_subject = Some(claims.subject.clone());
        let requested_directory = request_directory(&request);
        if let Some(directory) = requested_directory.as_deref() {
            if !crate::caller::allows_directory(&claims, &directory) {
                return auth_error(
                    StatusCode::FORBIDDEN,
                    "auth.directory_forbidden",
                    "The caller is not authorized for this directory",
                );
            }
        }
        if claims.hosted && matches!(operation_class(&request), OperationClass::HostedUnsupported | OperationClass::ManagementRead | OperationClass::ManagementMutation) {
            return auth_error(
                StatusCode::FORBIDDEN,
                "auth.hosted_route_forbidden",
                "This global credential or configuration route is unavailable in hosted mode",
            );
        }
        let query_session_id = request_session_id(request.uri());
        if claims.hosted
            && request.uri().path() == "/v2/events"
            && query_session_id.is_none()
        {
            return auth_error(
                StatusCode::BAD_REQUEST,
                "auth.session_scope_required",
                "Hosted event streams require sessionId",
            );
        }
        let plugin_path = request.uri().path().starts_with("/v2/plugins/");
        let matched_plugin_session = if plugin_path {
            match resolve_scoped_plugin_session(&state, request.uri().path(), request.method().as_str(), &claims).await {
                Ok(matched) => matched,
                Err(response) => return response,
            }
        } else {
            None
        };
        let mut owned_session = if plugin_path {
            matched_plugin_session.as_ref().map(|matched| matched.session_id.clone()).or(query_session_id)
        } else {
            session_id_from_path(request.uri().path()).map(str::to_string).or(query_session_id)
        };
        if owned_session.is_none() {
            if let Some(request_id) = interaction_id_from_path(request.uri().path()) {
                match state.inner.store.interaction_session_id(request_id).await {
                    Ok(session_id) => owned_session = session_id,
                    Err(error) => {
                        tracing::warn!(%error, "failed to authorize interaction owner");
                        return auth_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "auth.lookup_failed",
                            "Failed to authorize the interaction",
                        );
                    }
                }
            }
        }
        let mut authorized_session = false;
        if let Some(session_id) = owned_session.as_deref() {
            match state.inner.store.get_session(session_id).await {
                Ok(Some(session)) => match allows_session_or_ancestor(&state, &claims, &session).await {
                    Ok(true) => authorized_session = true,
                    Ok(false) => {
                        return auth_error(
                            StatusCode::FORBIDDEN,
                            "auth.session_forbidden",
                            "The caller is not authorized for this session or directory",
                        );
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to authorize session ancestry");
                        return auth_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "auth.lookup_failed",
                            "Failed to authorize the session",
                        );
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, "failed to authorize session owner");
                    return auth_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "auth.lookup_failed",
                        "Failed to authorize the session",
                    );
                }
                _ => {}
            }
        }
        if claims.hosted
            && requested_directory.is_none()
            && !authorized_session
            && requires_directory_scope(request.uri().path())
        {
            return auth_error(StatusCode::BAD_REQUEST, "auth.directory_scope_required", "This hosted route requires an authorized session or directory scope");
        }
        if let Some(matched) = matched_plugin_session { request.extensions_mut().insert(matched); }
        request.extensions_mut().insert(claims);
    }
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if let Some(tenant_id) = audit_tenant {
        let entry = neoism_agent_core::AuditEntry {
            id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit).to_string(),
            tenant_id,
            subject: audit_subject,
            method,
            path,
            status: response.status().as_u16(),
            created: crate::now_millis(),
        };
        if let Err(error) = state.inner.store.append_audit(&entry).await {
            tracing::warn!(%error, "failed to append hosted audit entry");
        }
    }
    drop(request_guard);
    response
}

fn is_mcp_oauth_callback_get(request: &Request<Body>) -> bool {
    request.method() == Method::GET
        && request.uri().path().starts_with("/v2/plugins/dev.neoism.mcp/")
        && request.uri().path().ends_with("/auth/callback")
        && url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
            .any(|(key, value)| key == "state" && !value.is_empty())
}

fn is_ticketed_pty_connect(request: &Request<Body>) -> bool {
    let path = request.uri().path();
    request.method() == Method::GET
        && path.starts_with("/v2/plugins/dev.neoism.pty/")
        && path.ends_with("/connect")
        && url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
            .any(|(key, value)| key == "ticket" && !value.is_empty())
}

#[cfg(test)]
mod ticket_auth_tests {
    use super::*;

    fn request(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[test]
    fn only_exact_ticketed_pty_connect_bypasses_bearer_auth() {
        assert!(is_ticketed_pty_connect(&request(
            "/v2/plugins/dev.neoism.pty/pty-1/connect?ticket=single-use"
        )));
        assert!(!is_ticketed_pty_connect(&request(
            "/v2/plugins/dev.neoism.pty/pty-1/connect"
        )));
        assert!(!is_ticketed_pty_connect(&request(
            "/v2/plugins/dev.neoism.pty/pty-1/connect?ticket="
        )));
        assert!(!is_ticketed_pty_connect(&request(
            "/v2/plugins/dev.example/pty-1/connect?ticket=single-use"
        )));
        assert!(!is_ticketed_pty_connect(&request(
            "/v2/plugins/dev.neoism.pty/pty-1?ticket=single-use"
        )));
    }

    #[test]
    fn only_state_bearing_mcp_oauth_get_callback_bypasses_bearer_auth() {
        assert!(is_mcp_oauth_callback_get(&request(
            "/v2/plugins/dev.neoism.mcp/github/auth/callback?code=abc&state=nonce"
        )));
        assert!(!is_mcp_oauth_callback_get(&request(
            "/v2/plugins/dev.neoism.mcp/github/auth/callback?code=abc"
        )));
        let post = Request::builder()
            .method(Method::POST)
            .uri("/v2/plugins/dev.neoism.mcp/github/auth/callback?state=nonce")
            .body(Body::empty())
            .unwrap();
        assert!(!is_mcp_oauth_callback_get(&post));
    }
}

fn auth_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "code": code,
            "message": message,
            "retryable": false,
            "details": {}
        })),
    )
        .into_response()
}

fn request_directory(request: &Request<Body>) -> Option<String> {
    // An explicit query is the route input and must never be hidden by a
    // transport-added default directory header. Both are still checked against
    // the signed caller prefixes.
    url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
        .find(|(key, _)| key == "directory")
        .map(|(_, value)| value.into_owned())
        .or_else(|| {
            request
                .headers()
                .get("x-neoism-directory")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
}

fn request_session_id(uri: &axum::http::Uri) -> Option<String> {
    url::form_urlencoded::parse(uri.query()?.as_bytes())
        .find(|(key, _)| key == "sessionId")
        .map(|(_, value)| value.into_owned())
}

fn session_id_from_path(path: &str) -> Option<&str> {
    // Plugin routes (/v2/plugins/...) are excluded on purpose: their session
    // scope is resolved via `find_scoped_plugin_session`, which validates the
    // segment against a RouteScope::Session descriptor instead of guessing
    // from position. Core routes like /v2/sessions/{id}/... resolve here.
    if path.starts_with("/v2/plugins/") {
        return None;
    }
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if let Some(index) = parts.iter().position(|part| *part == "sessions") {
        let id = *parts.get(index + 1)?;
        return (!matches!(id, "status" | "workspace" | "project")).then_some(id);
    }
    None
}

fn interaction_id_from_path(path: &str) -> Option<&str> {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let index = parts
        .iter()
        .position(|part| *part == "permissions" || *part == "questions")?;
    parts.get(index + 1).copied()
}

fn hosted_restricted_path(path: &str) -> bool {
    matches!(
        path,
        "/v2/config" | "/v2/config/validate"
    ) || (path.starts_with("/v2/providers/")
            && (path.ends_with("/auth") || path.contains("/oauth/")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationClass { Standard, HostedUnsupported, ManagementRead, ManagementMutation }

fn operation_class(request: &Request<Body>) -> OperationClass {
    let path = request.uri().path();
    if (path == "/v2/plugins/dev.neoism.workflows"
        && request.method() == Method::POST)
        || (path.starts_with("/v2/plugins/dev.neoism.workflows/")
            && !path.ends_with("/activate")
            && !path.ends_with("/pause")
            && !path.ends_with("/run")
            && !path.contains("/runs/")
            && matches!(*request.method(), Method::PUT | Method::PATCH | Method::DELETE))
    {
        return OperationClass::ManagementMutation;
    }
    if path.starts_with("/v2/management/") {
        return if matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
            OperationClass::ManagementRead
        } else {
            OperationClass::ManagementMutation
        };
    }
    if hosted_restricted_path(path) { OperationClass::HostedUnsupported } else { OperationClass::Standard }
}

fn requires_directory_scope(path: &str) -> bool {
    !path.starts_with("/v2/sessions")
        && !path.starts_with("/v2/interactions")
        && !path.starts_with("/v2/artifacts")
        && !path.starts_with("/v2/events")
        && !path.starts_with("/v2/audit")
        && !path.starts_with("/v2/meta")
        && !path.starts_with("/v2/openapi")
        && !path.starts_with("/v2/capabilities")
        && path != "/v2/health"
}

#[cfg(test)]
mod hosted_plugin_authorization_tests {
    use super::*;

    struct NoopRoute;
    impl neoism_agent_plugin_api::RouteHandler for NoopRoute {
        fn handle<'a>(&'a self, _: neoism_agent_plugin_api::RouteRequest) -> neoism_agent_plugin_api::PluginFuture<'a, neoism_agent_plugin_api::RouteResponse> {
            Box::pin(async { Ok(neoism_agent_plugin_api::RouteResponse::json(200, serde_json::Value::Null)) })
        }
    }

    fn route(id: &str, path: &str, scope: neoism_agent_plugin_api::RouteScope) -> neoism_agent_plugin_api::RegisteredRouteContribution {
        neoism_agent_plugin_api::RegisteredRouteContribution {
            plugin_id: "dev.example.route-auth".into(),
            route: neoism_agent_plugin_api::RouteContribution {
                metadata: neoism_agent_plugin_api::ContributionMetadata::new(id, "dev.example.route-auth", neoism_agent_plugin_api::PluginScope::Workspace),
                descriptor: neoism_agent_plugin_api::RouteDescriptor { id: id.into(), method: neoism_agent_plugin_api::RouteMethod::Get, path: path.into(), scope, request_schema: None, response_schema: None },
                handler: std::sync::Arc::new(NoopRoute),
            },
        }
    }

    #[test]
    fn hosted_plugin_session_fallback_requires_a_matched_session_descriptor() {
        assert!(requires_directory_scope("/v2/plugins/dev.example/items"));
        let mut snapshot = neoism_agent_plugin_api::RegistrySnapshot::empty();
        snapshot.runtime_routes.insert("workspace".into(), route("workspace", "/v2/plugins/dev.example.route-auth/:resource_id", neoism_agent_plugin_api::RouteScope::Workspace));
        assert!(!snapshot_matches_session_route(&snapshot, "GET", "/v2/plugins/dev.example.route-auth/ses_fake", "ses_fake"));
        snapshot.runtime_routes.insert("session".into(), route("session", "/v2/plugins/dev.example.route-auth/sessions/:session_id", neoism_agent_plugin_api::RouteScope::Session));
        assert!(snapshot_matches_session_route(&snapshot, "GET", "/v2/plugins/dev.example.route-auth/sessions/ses_real", "ses_real"));
    }

    #[test]
    fn hosted_clients_can_read_safe_defaults_but_not_full_config() {
        assert!(hosted_restricted_path("/v2/config"));
        assert!(hosted_restricted_path("/v2/config/validate"));
        assert!(!hosted_restricted_path("/v2/config/defaults"));
    }

    fn scoped_claims(directory: String, tenant_id: &str) -> crate::caller::CallerClaims {
        crate::caller::CallerClaims {
            subject: "scoped-test".into(), workspace_id: None, tenant_id: tenant_id.into(),
            directory_prefixes: vec![directory], hosted: false, max_sessions: None,
            max_artifacts: None, max_artifact_bytes: None, artifact_retention_days: None,
            requests_per_minute: None, max_in_flight: None,
        }
    }

    async fn session_route_fixture() -> (AppState, neoism_agent_core::SessionInfo, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("neoism-plugin-route-auth-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let now = crate::now_millis();
        let mut extra = std::collections::BTreeMap::new();
        extra.insert(crate::caller::TENANT_EXTRA_KEY.into(), serde_json::Value::String("tenant-a".into()));
        let session = neoism_agent_core::SessionInfo {
            id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Session),
            slug: "route-auth".into(), project_id: "global".into(), workspace_id: None,
            directory: root.to_string_lossy().into_owned(), path: None, parent_id: None,
            title: "Route auth".into(), agent: None, model: None,
            version: env!("CARGO_PKG_VERSION").into(),
            time: neoism_agent_core::TimeInfo { created: now, updated: now, compacting: None, archived: None },
            permission: None, extra,
        };
        state.inner.store.insert_session(&session).await.unwrap();
        (state, session, root)
    }

    #[test]
    fn session_id_from_path_never_guesses_on_plugin_routes() {
        assert_eq!(session_id_from_path("/v2/plugins/dev.neoism.goals/ses_123"), None);
        assert_eq!(
            session_id_from_path("/v2/plugins/dev.neoism.mcp/ses_123/tools"),
            None
        );
        assert_eq!(
            session_id_from_path("/v2/plugins/dev.neoism.subagents/sessions/ses_123/tasks"),
            None
        );
        assert_eq!(
            session_id_from_path("/v2/sessions/ses_123/messages"),
            Some("ses_123")
        );
    }

    #[tokio::test]
    async fn workspace_scoped_plugin_route_ignores_colliding_session_id_segment() {
        let (state, session, root) = session_route_fixture().await;
        // The MCP tools route binds `:name`, not `:session_id`, and is
        // workspace-scoped — a server name that collides with a real session
        // id must not resolve dispatch into that session's workspace.
        let path = format!("/v2/plugins/dev.neoism.mcp/{}/tools", session.id);
        assert!(find_scoped_plugin_session(&state, &path, "GET").await.is_none());
        // The same session id on a genuinely session-scoped route resolves.
        let scoped = format!(
            "/v2/plugins/{}/{}",
            neoism_agent_builtins::plugin::goals::ID,
            session.id
        );
        let matched = find_scoped_plugin_session(&state, &scoped, "GET")
            .await
            .expect("session-scoped descriptor must resolve");
        assert_eq!(matched.id, session.id);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn denied_scoped_non_hosted_credential_cannot_use_session_plugin_route() {
        let (state, session, root) = session_route_fixture().await;
        let path = format!("/v2/plugins/{}/{}", neoism_agent_builtins::plugin::goals::ID, session.id);
        let denied = scoped_claims(root.to_string_lossy().into_owned(), "tenant-b");
        let response = resolve_scoped_plugin_session(&state, &path, "GET", &denied).await.unwrap_err();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn allowed_scoped_non_hosted_credential_resolves_session_plugin_route() {
        let (state, session, root) = session_route_fixture().await;
        let path = format!("/v2/plugins/{}/{}", neoism_agent_builtins::plugin::goals::ID, session.id);
        let allowed = scoped_claims(root.to_string_lossy().into_owned(), "tenant-a");
        let matched = resolve_scoped_plugin_session(&state, &path, "GET", &allowed).await.unwrap().unwrap();
        assert_eq!(matched.session_id, session.id.to_string());
        assert_eq!(crate::workspace_runtime::canonical_location(&matched.directory), crate::workspace_runtime::canonical_location(&session.directory));
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn shared_parent_authorizes_legacy_child_without_tenant_marker() {
        let (state, mut parent, root) = session_route_fixture().await;
        parent.workspace_id = Some("workspace-a".into());
        state.inner.store.update_session(&parent).await.unwrap();
        let mut child = parent.clone();
        child.id = neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Session);
        child.parent_id = Some(parent.id.clone());
        child.title = "Legacy subagent".into();
        child.extra.clear();
        state.inner.store.insert_session(&child).await.unwrap();

        let mut allowed = scoped_claims(root.to_string_lossy().into_owned(), "tenant-a");
        allowed.workspace_id = Some("workspace-a".into());
        assert!(!crate::caller::allows_session(&allowed, &child));
        assert!(allows_session_or_ancestor(&state, &allowed, &child)
            .await
            .unwrap());

        let denied = scoped_claims(root.to_string_lossy().into_owned(), "tenant-b");
        assert!(!allows_session_or_ancestor(&state, &denied, &child)
            .await
            .unwrap());
        let mut wrong_workspace = allowed.clone();
        wrong_workspace.workspace_id = Some("workspace-b".into());
        assert!(!allows_session_or_ancestor(&state, &wrong_workspace, &child)
            .await
            .unwrap());

        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
