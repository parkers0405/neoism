use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower::ServiceExt;
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
use crate::lsp_routes::{
    lsp_code_actions, lsp_definition, lsp_diagnostics, lsp_document_highlights,
    lsp_document_symbols, lsp_formatting, lsp_hover, lsp_implementation,
    lsp_incoming_calls, lsp_inlay_hints, lsp_outgoing_calls, lsp_prepare_call_hierarchy,
    lsp_references, lsp_shutdown, lsp_signature_help, lsp_status, lsp_touch,
};
use crate::mcp_routes::{
    mcp_add, mcp_auth_authenticate, mcp_auth_callback, mcp_auth_callback_get,
    mcp_auth_remove, mcp_auth_start, mcp_catalog, mcp_config_patch, mcp_connect,
    mcp_disconnect, mcp_prompts, mcp_resources, mcp_status, mcp_tool_call, mcp_tools,
};
use crate::openapi::canonical_openapi_doc;
use crate::pty_routes::{
    pty_connect, pty_connect_token, pty_create, pty_get, pty_list, pty_remove,
    pty_shells, pty_update,
};
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
    v2_wait,
};
use crate::workflow::{
    workflow_activate, workflow_get, workflow_history, workflow_list, workflow_pause,
    workflow_preview, workflow_run_now,
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
        .route("/v2/plugins/:plugin_id", get(v2_plugin))
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

async fn plugin_route_dispatch(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let directory = if let Some(directory) = request_directory(&request) {
        directory
    } else if let Some(session_id) = session_id_from_path(request.uri().path()) {
        match state.inner.store.get_session(session_id).await {
            Ok(Some(session)) => session.directory,
            _ => return StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned()
    };
    let snapshot = state.plugin_snapshot(&directory).await;
    let runtime_route = snapshot.runtime_routes.values().find_map(|registered| {
        (registered.route.descriptor.method.as_str() == request.method().as_str())
            .then(|| match_plugin_path(&registered.route.descriptor.path, request.uri().path()))
            .flatten()
            .map(|params| (registered, params))
    });
    if let Some((registered, path_params)) = runtime_route {
        return dispatch_runtime_route(registered, path_params, directory, request).await;
    }
    plugin_router(&snapshot, state)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {})
}

fn plugin_router(snapshot: &neoism_agent_plugin_api::RegistrySnapshot, state: AppState) -> Router {
    let route = |id: &str| snapshot.contributions.contains_key(&format!("Route:{id}"));
    let mut router = Router::new();
    if route("subagents") { router = router.route("/v2/plugins/dev.neoism.subagents/sessions/:session_id/tasks", get(crate::plugins::subagents::list_tasks)).route("/v2/plugins/dev.neoism.subagents/sessions/:session_id/stop", post(crate::plugins::subagents::stop_tasks)); }
    if route("workflows") { router = router.route("/v2/plugins/dev.neoism.workflows", get(workflow_list)).route("/v2/plugins/dev.neoism.workflows/:workflow_id", get(workflow_get)).route("/v2/plugins/dev.neoism.workflows/:workflow_id/activate", post(workflow_activate)).route("/v2/plugins/dev.neoism.workflows/:workflow_id/pause", post(workflow_pause)).route("/v2/plugins/dev.neoism.workflows/:workflow_id/run", post(workflow_run_now)).route("/v2/plugins/dev.neoism.workflows/:workflow_id/preview", get(workflow_preview)).route("/v2/plugins/dev.neoism.workflows/:workflow_id/runs", get(workflow_history)); }
    if route("lsp") { router = router.route("/v2/plugins/dev.neoism.lsp", get(lsp_status)).route("/v2/plugins/dev.neoism.lsp/hover", get(lsp_hover)).route("/v2/plugins/dev.neoism.lsp/signature-help", get(lsp_signature_help)).route("/v2/plugins/dev.neoism.lsp/inlay-hints", get(lsp_inlay_hints)).route("/v2/plugins/dev.neoism.lsp/document-highlights", get(lsp_document_highlights)).route("/v2/plugins/dev.neoism.lsp/definition", get(lsp_definition)).route("/v2/plugins/dev.neoism.lsp/references", get(lsp_references)).route("/v2/plugins/dev.neoism.lsp/implementation", get(lsp_implementation)).route("/v2/plugins/dev.neoism.lsp/prepare-call-hierarchy", get(lsp_prepare_call_hierarchy)).route("/v2/plugins/dev.neoism.lsp/incoming-calls", get(lsp_incoming_calls)).route("/v2/plugins/dev.neoism.lsp/outgoing-calls", get(lsp_outgoing_calls)).route("/v2/plugins/dev.neoism.lsp/diagnostics", get(lsp_diagnostics)).route("/v2/plugins/dev.neoism.lsp/document-symbols", get(lsp_document_symbols)).route("/v2/plugins/dev.neoism.lsp/formatting", get(lsp_formatting)).route("/v2/plugins/dev.neoism.lsp/code-actions", get(lsp_code_actions)).route("/v2/plugins/dev.neoism.lsp/touch", post(lsp_touch)).route("/v2/plugins/dev.neoism.lsp/shutdown", post(lsp_shutdown)); }
    if route("pty") { router = router.route("/v2/plugins/dev.neoism.pty/shells", get(pty_shells)).route("/v2/plugins/dev.neoism.pty", get(pty_list).post(pty_create)).route("/v2/plugins/dev.neoism.pty/:pty_id", get(pty_get).put(pty_update).delete(pty_remove)).route("/v2/plugins/dev.neoism.pty/:pty_id/connect-token", post(pty_connect_token)).route("/v2/plugins/dev.neoism.pty/:pty_id/connect", get(pty_connect)); }
    if route("mcp") { router = router.route("/v2/plugins/dev.neoism.mcp", get(mcp_status).post(mcp_add)).route("/v2/plugins/dev.neoism.mcp/catalog", get(mcp_catalog)).route("/v2/plugins/dev.neoism.mcp/:name/auth", post(mcp_auth_start).delete(mcp_auth_remove)).route("/v2/plugins/dev.neoism.mcp/:name/auth/callback", get(mcp_auth_callback_get).post(mcp_auth_callback)).route("/v2/plugins/dev.neoism.mcp/:name/auth/authenticate", post(mcp_auth_authenticate)).route("/v2/plugins/dev.neoism.mcp/:name/connect", post(mcp_connect)).route("/v2/plugins/dev.neoism.mcp/:name/disconnect", post(mcp_disconnect)).route("/v2/plugins/dev.neoism.mcp/:name/config", patch(mcp_config_patch)).route("/v2/plugins/dev.neoism.mcp/:name/tools", get(mcp_tools)).route("/v2/plugins/dev.neoism.mcp/:name/tools/:tool_name", post(mcp_tool_call)).route("/v2/plugins/dev.neoism.mcp/:name/resources", get(mcp_resources)).route("/v2/plugins/dev.neoism.mcp/:name/prompts", get(mcp_prompts)); }
    router.with_state(state)
}

async fn dispatch_runtime_route(
    registered: &neoism_agent_plugin_api::RegisteredRouteContribution,
    path_params: std::collections::BTreeMap<String, String>,
    directory: String,
    request: Request<Body>,
) -> Response {
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
    let workspace_id = claims.and_then(|claims| claims.workspace_id.clone());
    let session_id = path_params.get("session_id").cloned();
    let request = neoism_agent_plugin_api::RouteRequest {
        workspace_id,
        workspace: Some(directory.into()),
        session_id,
        actor,
        path: path_params,
        query,
        headers,
        body,
    };
    match registered.route.handler.handle(request).await {
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

async fn authenticate_request(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS || request.uri().path() == "/v2/health" {
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
        if claims.hosted && hosted_restricted_path(request.uri().path()) {
            return auth_error(
                StatusCode::FORBIDDEN,
                "auth.hosted_route_forbidden",
                "This global credential or configuration route is unavailable in hosted mode",
            );
        }
        if claims.hosted
            && !claims.directory_prefixes.is_empty()
            && requested_directory.is_none()
            && requires_directory_scope(request.uri().path())
        {
            return auth_error(
                StatusCode::BAD_REQUEST,
                "auth.directory_scope_required",
                "This hosted route requires a directory scope",
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
        let mut owned_session = session_id_from_path(request.uri().path())
            .map(str::to_string)
            .or(query_session_id);
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
        if let Some(session_id) = owned_session.as_deref() {
            match state.inner.store.get_session(session_id).await {
                Ok(Some(session)) if !crate::caller::allows_session(&claims, &session) => {
                    return auth_error(
                        StatusCode::FORBIDDEN,
                        "auth.session_forbidden",
                        "The caller is not authorized for this session or directory",
                    );
                }
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
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let index = parts
        .iter()
        .position(|part| *part == "sessions")?;
    let id = *parts.get(index + 1)?;
    (!matches!(id, "status" | "workspace" | "project")).then_some(id)
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

fn requires_directory_scope(path: &str) -> bool {
    !path.starts_with("/v2/sessions")
        && !path.starts_with("/v2/interactions")
        && !path.starts_with("/v2/artifacts")
        && !path.starts_with("/v2/events")
        && !path.starts_with("/v2/audit")
        && !path.starts_with("/v2/meta")
        && !path.starts_with("/v2/openapi")
        && !path.starts_with("/v2/capabilities")
        && !path.starts_with("/v2/plugins")
        && path != "/v2/health"
}
