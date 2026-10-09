//! Optional host boundary: all authority comes from the caller's injected policy.
use crate::*;
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use neoism_cloud_runtime::{MachineHandle, WorkspaceKey, WorkspaceSpec};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAction {
    Ensure,
    Status,
    Start,
    Stop,
    Destroy,
    Connection,
}
/// Policy must validate the opaque bearer, bind the requested workspace to a
/// tenant, and explicitly authorize this verb. No permissive fallback exists.
pub trait HostPolicy: Send + Sync {
    fn authorize<'a>(
        &'a self,
        bearer: &'a str,
        workspace: &'a str,
        action: HostAction,
    ) -> Pin<Box<dyn Future<Output = Result<TrustedApproval>> + Send + 'a>>;
}
#[derive(Clone, Debug)]
pub struct TrustedApproval {
    pub key: WorkspaceKey,
    pub spec: WorkspaceSpec,
    pub access: HostApprovedAccess,
}
/// Reference wiring only; denies every request including status.
pub struct DenyAllPolicy;
impl HostPolicy for DenyAllPolicy {
    fn authorize<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: HostAction,
    ) -> Pin<Box<dyn Future<Output = Result<TrustedApproval>> + Send + 'a>> {
        Box::pin(async { Err(HostError::Denied) })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleRequest {
    pub expected_handle: MachineHandle,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostApiError {
    pub code: String,
}
#[derive(Clone)]
struct ApiState {
    host: Arc<WorkspaceHost>,
    policy: Arc<dyn HostPolicy>,
}
pub fn host_router(host: Arc<WorkspaceHost>, policy: Arc<dyn HostPolicy>) -> Router {
    Router::new()
        .route("/v1/workspaces/:workspace/runtime/status", get(status))
        .route("/v1/workspaces/:workspace/runtime/ensure", post(ensure))
        .route("/v1/workspaces/:workspace/runtime/start", post(start))
        .route("/v1/workspaces/:workspace/runtime/stop", post(stop))
        .route("/v1/workspaces/:workspace/runtime/destroy", post(destroy))
        .route(
            "/v1/workspaces/:workspace/runtime/connection",
            post(connection),
        )
        .layer(DefaultBodyLimit::max(16_384))
        .with_state(ApiState { host, policy })
}
impl IntoResponse for HostError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            HostError::Denied => (StatusCode::FORBIDDEN, "denied"),
            HostError::Invalid => (StatusCode::BAD_REQUEST, "invalid"),
            HostError::Stale
            | HostError::Runtime(
                neoism_cloud_runtime::Error::Fenced
                | neoism_cloud_runtime::Error::Conflict
                | neoism_cloud_runtime::Error::Busy,
            ) => (StatusCode::CONFLICT, "conflict"),
            HostError::Expired
            | HostError::Runtime(neoism_cloud_runtime::Error::ExpiredLaunch) => {
                (StatusCode::CONFLICT, "expired")
            }
            HostError::Unready | HostError::Timeout => {
                (StatusCode::SERVICE_UNAVAILABLE, "unready")
            }
            _ => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        };
        (
            status,
            [("cache-control", "no-store")],
            Json(HostApiError { code: code.into() }),
        )
            .into_response()
    }
}
async fn approve(
    state: &ApiState,
    headers: &HeaderMap,
    workspace: &str,
    action: HostAction,
) -> Result<TrustedApproval> {
    let mut values = headers.get_all(axum::http::header::AUTHORIZATION).iter();
    let raw = values
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(HostError::Denied)?;
    if values.next().is_some() {
        return Err(HostError::Denied);
    }
    let bearer = raw
        .strip_prefix("Bearer ")
        .filter(|v| {
            !v.is_empty() && v.len() <= 16_384 && !v.chars().any(char::is_whitespace)
        })
        .ok_or(HostError::Denied)?;
    let approval = state.policy.authorize(bearer, workspace, action).await?;
    approval.key.validate()?;
    approval.spec.validate()?;
    if approval.key.workspace != workspace {
        return Err(HostError::Denied);
    }
    Ok(approval)
}
fn reply<T: Serialize>(value: T) -> Response {
    ([("cache-control", "no-store")], Json(value)).into_response()
}
async fn status(
    State(s): State<ApiState>,
    Path(w): Path<String>,
    h: HeaderMap,
) -> Result<Response> {
    let a = approve(&s, &h, &w, HostAction::Status).await?;
    Ok(reply(s.host.status(&a.key)?))
}
async fn ensure(
    State(s): State<ApiState>,
    Path(w): Path<String>,
    h: HeaderMap,
    body: std::result::Result<
        Json<EmptyRequest>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<Response> {
    let Json(_) = body.map_err(|_| HostError::Invalid)?;
    let a = approve(&s, &h, &w, HostAction::Ensure).await?;
    Ok(reply(s.host.ensure_worker(&a.key, &a.spec).await?))
}
async fn connection(
    State(s): State<ApiState>,
    Path(w): Path<String>,
    h: HeaderMap,
    body: std::result::Result<
        Json<EmptyRequest>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<Response> {
    let Json(_) = body.map_err(|_| HostError::Invalid)?;
    let a = approve(&s, &h, &w, HostAction::Connection).await?;
    Ok(reply(s.host.connect(&a.key, &a.access).await?))
}
async fn lifecycle(
    s: ApiState,
    w: String,
    h: HeaderMap,
    request: LifecycleRequest,
    action: HostAction,
) -> Result<Response> {
    let a = approve(&s, &h, &w, action).await?;
    if request.expected_handle.owner != a.key {
        return Err(HostError::Denied);
    }
    let result = match action {
        HostAction::Start => s.host.start(&request.expected_handle).await?,
        HostAction::Stop => s.host.stop(&request.expected_handle).await?,
        HostAction::Destroy => s.host.destroy(&request.expected_handle).await?,
        _ => return Err(HostError::Denied),
    };
    Ok(reply(result))
}
macro_rules! verb {
    ($name:ident, $action:ident) => {
        async fn $name(
            State(s): State<ApiState>,
            Path(w): Path<String>,
            h: HeaderMap,
            body: std::result::Result<
                Json<LifecycleRequest>,
                axum::extract::rejection::JsonRejection,
            >,
        ) -> Result<Response> {
            let Json(r) = body.map_err(|_| HostError::Invalid)?;
            lifecycle(s, w, h, r, HostAction::$action).await
        }
    };
}
verb!(start, Start);
verb!(stop, Stop);
verb!(destroy, Destroy);

/// Authoritative host API schema consumed by SDK generation. Runtime schemas are
/// imported from their owner, not copied into a second handwritten contract.
pub fn canonical_openapi() -> serde_json::Value {
    use serde_json::{json, Value};
    fn reference(s: &str) -> Value {
        json!({"$ref": format!("#/components/schemas/{s}")})
    }
    fn object(props: Value) -> Value {
        let required: Vec<_> = props.as_object().unwrap().keys().cloned().collect();
        json!({"type":"object","additionalProperties":false,"properties":props,"required":required})
    }
    let mut schemas = neoism_cloud_runtime::canonical_openapi()["components"]["schemas"]
        .as_object()
        .unwrap()
        .clone();
    let integer = json!({"type":"integer","format":"int64"});
    schemas.insert("EmptyRequest".into(), object(json!({})));
    schemas.insert(
        "LifecycleRequest".into(),
        object(json!({"expected_handle":reference("MachineHandle")})),
    );
    schemas.insert("HostApiError".into(), object(json!({"code":{"type":"string","enum":["denied","invalid","conflict","expired","unready","unavailable"]}})));
    schemas.insert("Verification".into(), object(json!({"at":integer,"expires_at":integer,"handle":reference("MachineHandle"),"descriptor":reference("WorkerLaunchDescriptor"),"revision":integer})));
    schemas.insert("HostStatus".into(), object(json!({"binding":reference("Binding"),"verification":{"anyOf":[reference("Verification"),{"type":"null"}]}})));
    schemas.insert("ConnectionGrant".into(), object(json!({"version":{"type":"integer","const":1},"baseUrl":{"type":"string","format":"uri"},"handle":reference("MachineHandle"),"root":{"type":"string"},"runtimeId":{"type":"string"},"workerGeneration":integer,"bearer":{"type":"string","description":"Opaque short-lived credential. Never log or persist."},"expiresAt":integer,"capabilities":reference("Capabilities")})));
    let mut paths = serde_json::Map::new();
    for action in ["ensure", "status", "start", "stop", "destroy", "connection"] {
        let mut operation = json!({"operationId":format!("host.runtime.{action}"),"security":[{"HostBearer":[]}],"parameters":[{"name":"workspace","in":"path","required":true,"schema":{"type":"string"}}],"responses":{}});
        if action != "status" {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":reference(if matches!(action,"start"|"stop"|"destroy") {"LifecycleRequest"} else {"EmptyRequest"})}}});
        }
        operation["responses"]["200"] = json!({"description":"Current snapshot (verification is ephemeral) or short-lived connection grant","content":{"application/json":{"schema":reference(if action == "connection" {"ConnectionGrant"} else {"HostStatus"})}}});
        for code in ["400", "403", "409", "503"] {
            operation["responses"][code] = json!({"description":"Sanitized host error","content":{"application/json":{"schema":reference("HostApiError")}}});
        }
        let mut entry = serde_json::Map::new();
        entry.insert(
            if action == "status" { "get" } else { "post" }.into(),
            operation,
        );
        paths.insert(
            format!("/v1/workspaces/{{workspace}}/runtime/{action}"),
            Value::Object(entry),
        );
    }
    json!({"openapi":"3.1.0","info":{"title":"Neoism Workspace Host API","version":"1.0.0"},"paths":paths,"components":{"schemas":schemas,"securitySchemes":{"HostBearer":{"type":"http","scheme":"bearer"}}}})
}
