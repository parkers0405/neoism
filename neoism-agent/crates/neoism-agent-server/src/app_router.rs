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
    v2_wait,
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
        )
        .await;
    }
    StatusCode::NOT_FOUND.into_response()
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
) -> Response {
    let (mut parts, _) = request.into_parts();
    let query = url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes())
        .fold(std::collections::BTreeMap::<String, Vec<String>>::new(), |mut output, (key, value)| {
            output.entry(key.into_owned()).or_default().push(value.into_owned());
            output
        });
    let headers = parts.headers.iter().filter_map(|(name, value)| value.to_str().ok().map(|value| (name.to_string(), value.to_string()))).collect();
    let claims = parts.extensions.get::<crate::caller::CallerClaims>();
    let route_request = neoism_agent_plugin_api::RouteRequest {
        workspace_id: claims.and_then(|claims| claims.workspace_id.clone()),
        workspace: Some(directory.into()),
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
    upgrade.on_upgrade(move |socket| async move {
        let _generation_lease = generation_lease;
        let _ = session.run(Box::new(HostWebSocket(socket))).await;
    }).into_response()
}

async fn dispatch_runtime_route(
    registered: &neoism_agent_plugin_api::RegisteredRouteContribution,
    path_params: std::collections::BTreeMap<String, String>,
    directory: String,
    request: Request<Body>,
    generation: crate::workspace_runtime::PluginGenerationLease,
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
