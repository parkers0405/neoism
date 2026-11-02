//! Client-only instruction projection. Never apply this to persistence or provider input.
use axum::body::{to_bytes, Body, HttpBody};
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

/// Project a historical or live event immediately before client delivery.
/// Public so daemon websocket adapters can use the same boundary policy.
pub fn project_client_event(event: &mut neoism_agent_core::EventPayload) {
    project_event_data(&event.kind, &mut event.properties);
}

pub(crate) fn project_event_data(kind: &str, data: &mut Value) {
    if kind == neoism_agent_core::event_type::SESSION_CONTEXT_UPDATED {
        if let Some(epoch) = data.get_mut("epoch") {
            *epoch = public_epoch(epoch);
        }
    }
    project_client_json(data);
}

/// Retain only known, typed epoch metadata; new private fields fail closed.
fn public_epoch(epoch: &Value) -> Value {
    let mut result = Map::new();
    for key in ["generation", "baselineSeq", "updated"] {
        if let Some(value) = epoch.get(key).filter(|value| value.is_u64()) {
            result.insert(key.into(), value.clone());
        }
    }
    for key in ["sources", "changedSources", "instructionSources"] {
        if let Some(values) = epoch.get(key).and_then(Value::as_array) {
            result.insert(
                key.into(),
                Value::Array(values.iter().filter(|v| v.is_string()).cloned().collect()),
            );
        }
    }
    // Old journal/session rows contain snapshots rather than public source names.
    if !result.contains_key("sources") {
        if let Some(sources) = epoch.pointer("/snapshot/sources").and_then(Value::as_object) {
            result.insert(
                "sources".into(),
                Value::Array(
                    sources.keys()
                        .filter(|key| !matches!(key.as_str(), "instructionState" | "instructionUpdates"))
                        .cloned().map(Value::String).collect(),
                ),
            );
        }
    }
    if !result.contains_key("instructionSources") {
        if let Some(loaded) = epoch
            .pointer("/snapshot/sources/instructionState/loaded")
            .and_then(Value::as_array)
        {
            result.insert(
                "instructionSources".into(),
                Value::Array(loaded.iter()
                    .filter_map(|entry| entry.get("filepath").filter(|v| v.is_string()).cloned())
                    .collect()),
            );
        }
    }
    Value::Object(result)
}

fn public_instructions(value: &Value) -> Value {
    Value::Array(value.as_array().into_iter().flatten().filter_map(|entry| {
        let mut result = Map::new();
        for key in ["filepath", "path"] {
            if let Some(value) = entry.get(key).filter(|v| v.is_string()) {
                result.insert(key.into(), value.clone());
            }
        }
        if let Some(value) = entry.get("loaded").filter(|v| v.is_boolean()) {
            result.insert("loaded".into(), value.clone());
        }
        (!result.is_empty()).then_some(Value::Object(result))
    }).collect())
}

/// Sanitize reserved transport fields, not ordinary file/tool/user data.
/// Opaque data fields are deliberately not traversed: reading AGENTS.md is a
/// normal user-requested read and its output must remain intact.
pub fn project_client_json(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(project_client_json),
        Value::Object(object) => {
            // Recognize both legacy payloads and v2 event envelopes.
            let kind = object.get("type")
                .or_else(|| object.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if kind.as_deref() == Some(neoism_agent_core::event_type::SESSION_CONTEXT_UPDATED) {
                for key in ["properties", "data"] {
                    if let Some(epoch) = object.get_mut(key).and_then(|data| data.get_mut("epoch")) {
                        *epoch = public_epoch(epoch);
                    }
                }
            }
            for (key, value) in object.iter_mut() {
                match key.as_str() {
                    "contextEpoch" => *value = public_epoch(value),
                    "instructionContext" => *value = public_instructions(value),
                    "output" | "input" | "arguments" | "text" | "content" => {}
                    _ => project_client_json(value),
                }
            }
        }
        _ => {}
    }
}

/// Axum Json responses have an exact-sized body. Never collect SSE, websocket,
/// or unknown-length plugin streams; those project at their event serializer.
pub(crate) async fn project_json_response(request: Request<Body>, next: Next) -> Response {
    // Artifact downloads are ordinary file data, even when their MIME is JSON.
    let artifact_content = request.uri().path().starts_with("/v2/artifacts/")
        && request.uri().path().ends_with("/content");
    let response = next.run(request).await;
    let json = response.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().is_some_and(|mime| mime.trim() == "application/json"));
    if artifact_content || !json || response.body().size_hint().exact().is_none() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    project_client_json(&mut value);
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    // Any length/digest describing the original body is no longer valid.
    for key in [header::CONTENT_LENGTH, header::ETAG] {
        parts.headers.remove(key);
    }
    parts.headers.remove("content-md5");
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn private_epoch() -> Value {
        json!({"generation":3,"baselineSeq":1,"updated":42,
            "baseline":{"sources":{"instructions":["SECRET baseline"]}},
            "snapshot":{"sources":{"instructions":["SECRET current"],"service:memory":"SECRET memory"}},
            "state":"SECRET state","futurePrivate":"SECRET future"})
    }

    #[test]
    fn projects_sessions_and_exports_without_mutating_source() {
        let source = json!({"bundles":[{"session":{"id":"ses_1","contextEpoch":private_epoch()}}]});
        let mut client = source.clone();
        project_client_json(&mut client);
        assert!(!client.to_string().contains("SECRET"));
        assert!(source.to_string().contains("SECRET"));
        assert_eq!(client["bundles"][0]["session"]["contextEpoch"], json!({
            "generation":3,"baselineSeq":1,"updated":42,"sources":["instructions","service:memory"]}));
        let again = client.clone();
        project_client_json(&mut client);
        assert_eq!(client, again);
    }

    #[test]
    fn projects_legacy_and_v2_historical_events() {
        for (kind, data) in [("type", "properties"), ("kind", "data")] {
            let mut event = json!({kind:"session.context.updated",data:{"sessionID":"ses_1","epoch":private_epoch()}});
            project_client_json(&mut event);
            assert!(!event.to_string().contains("SECRET"));
            assert_eq!(event[data]["epoch"]["generation"], 3);
        }
        let mut event = neoism_agent_core::EventPayload::new("session.context.updated", json!({"epoch":private_epoch()}));
        project_client_event(&mut event);
        assert!(!event.properties.to_string().contains("SECRET"));
    }

    #[test]
    fn preserves_public_source_names_and_projects_private_loaded_state() {
        let public = json!({"contextEpoch":{"generation":2,"sources":["instructions"],
            "instructionSources":["/repo/AGENTS.md","https://example.org/rules"],"changedSources":["instructions"]}});
        let mut value = public.clone();
        project_client_json(&mut value);
        assert_eq!(value, public);
        let mut value = json!({"contextEpoch":{"snapshot":{"sources":{
            "instructionState":{"loaded":[{"filepath":"/repo/AGENTS.md","content":"SECRET"}]},
            "instructionUpdates":["SECRET"],"instructions":["SECRET"]}}}});
        project_client_json(&mut value);
        assert_eq!(value["contextEpoch"], json!({"sources":["instructions"],"instructionSources":["/repo/AGENTS.md"]}));
    }

    #[test]
    fn tool_metadata_keeps_paths_and_actual_read_output() {
        let output = json!({"instructionContext":[{"filepath":"file","content":"ordinary file data"}],"contextEpoch":private_epoch()});
        let mut part = json!({"state":{"metadata":{"instructionContext":[{
            "filepath":"/project/AGENTS.md","loaded":true,"content":"SECRET instruction","state":"SECRET state"
        }],"loaded":["/project/AGENTS.md"]},"output":output,"input":{"content":"ordinary input"}}});
        project_client_json(&mut part);
        assert_eq!(part["state"]["output"], output);
        assert_eq!(part["state"]["metadata"]["instructionContext"], json!([{"filepath":"/project/AGENTS.md","loaded":true}]));
        assert_eq!(part["state"]["metadata"]["loaded"], json!(["/project/AGENTS.md"]));
    }

    #[tokio::test]
    async fn http_projects_json_and_leaves_sse_uncollected() {
        use axum::{routing::get, Json, Router};
        use tower::ServiceExt;
        let app = Router::new()
            .route("/session", get(|| async { Json(json!({"contextEpoch":private_epoch()})) }))
            .route("/v2/sessions/export", get(|| async { Json(json!({"bundles":[{"session":{"contextEpoch":private_epoch()},
                "messages":[{"parts":[{"state":{"metadata":{"instructionContext":[{"filepath":"/repo/AGENTS.md","content":"SECRET"}]},
                    "output":"user-requested read output"}}]}]}]})) }))
            .route("/v2/artifacts/file/content", get(|| async { Json(json!({"instructionContext":[{"content":"ordinary file data"}]})) }))
            .route("/events", get(|| async {
                axum::response::Sse::new(futures::stream::pending::<Result<axum::response::sse::Event, std::convert::Infallible>>())
            }))
            .route("/json-stream", get(|| async {
                let stream = futures::stream::pending::<Result<axum::body::Bytes, std::convert::Infallible>>();
                ([(header::CONTENT_TYPE, "application/json")], Body::from_stream(stream))
            }))
            .layer(axum::middleware::from_fn(project_json_response));
        let response = app.clone().oneshot(Request::builder().uri("/session").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("SECRET"));
        let response = app.clone().oneshot(Request::builder().uri("/v2/sessions/export").body(Body::empty()).unwrap()).await.unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("SECRET"));
        assert!(body.contains("/repo/AGENTS.md"));
        assert!(body.contains("user-requested read output"));
        let response = app.clone().oneshot(Request::builder().uri("/v2/artifacts/file/content").body(Body::empty()).unwrap()).await.unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("ordinary file data"));
        for (path, mime) in [("/events", "text/event-stream"), ("/json-stream", "application/json")] {
            let response = tokio::time::timeout(std::time::Duration::from_secs(1), app.clone().oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())).await.unwrap().unwrap();
            assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
        }
    }
}